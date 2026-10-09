//! Native item/project authority. All mutation and embed calls are main-thread-only.
//! Lifecycle state is lossless; only discrete host notifications have a bounded FIFO.
use crate::{
    config::{Settings, SettingsError, SettingsState, StatusSource, ToolIcon, ToolKind, ToolSpec},
    discovery::{Conversation, ConversationList, StoreError},
    launch,
    platform::{self, MainWindow},
    runtime::{self, ConfigUpdate, Request},
    shortcuts::{Action, Key, Keymap, Mods},
    status::{Activity, AgentTracker, Attention, OmpEvent},
    ui_state::{self, UiStateFile},
};
use alacritty_embed::embed::{self, Launch, SessionKey, TerminalState, ViewportRect};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap, VecDeque},
    ffi::{c_void, OsStr},
    path::{Path, PathBuf},
    rc::Rc,
};
use tauri::{AppHandle, Emitter, Manager};

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
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForegroundJobState {
    /// The group leader's command name (`cargo`, `ssh`).
    pub command: String,
    pub since_ms: u64,
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
    label: String,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    tracker: Option<AgentTracker>,
    raw_title: String,
    conversation_title: Option<String>,
    closing: bool,
    extra: Option<Vec<String>>,
    /// A foreground group seen on the last poll but not yet published: (pgid, command, first seen).
    job_candidate: Option<(i32, String, u64)>,
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
    fn prepare(&self, id: String, spec: ToolSpec, cwd: String, args: Vec<String>);
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
    fn notify(&self, summary: &str, body: &str);
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
        if let Err(e) = self.app.emit(WORKSPACE_STATE_EVENT, state) {
            log::warn!("workspace event: {e}");
        }
    }
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self.app.emit(event, payload);
    }
    fn prepare(&self, id: String, spec: ToolSpec, cwd: String, args: Vec<String>) {
        std::thread::spawn(move || {
            let prepared = launch::prepare(&spec, &cwd, args);
            platform::defer_on_main(Box::new(move || finish_launch(&id, prepared)));
        });
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
        self.app
            .get_webview_window(crate::lifecycle::MAIN_WINDOW)
            .is_some_and(|win| win.is_focused().unwrap_or(false))
    }
    fn notify(&self, summary: &str, body: &str) {
        crate::notify::desktop(summary, body);
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
    Notify(String),
    Bell,
}
#[derive(Default)]
struct HostQueue {
    events: VecDeque<(SessionKey, HostUpdate)>,
    discrete: usize,
}
impl HostQueue {
    fn push(&mut self, key: SessionKey, update: HostUpdate) {
        if !matches!(update, HostUpdate::State(_)) {
            if self.discrete == 512 {
                // Never evict lifecycle state, even during a burst of bells/notifications.
                let oldest = self
                    .events
                    .iter()
                    .position(|(_, event)| !matches!(event, HostUpdate::State(_)))
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
            Some((w.host.clone(), w.snapshot()))
        } else {
            None
        };
        let conversations = (discovery_revision != w.discovery_revision)
            .then(|| (w.host.clone(), w.discovery_revision));
        Ok::<_, String>((result, emit, conversations))
    })?;
    if let Some((host, state)) = emit {
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
#[cfg(target_os = "macos")]
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
                    if record.state.title != title || record.state.model != c.model {
                        record.state.title = title;
                        record.state.model.clone_from(&c.model);
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
        let tracker = (spec.status == StatusSource::Omp).then(AgentTracker::new);
        let mut record = Record {
            key: None,
            spec: spec.clone(),
            tracker,
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
        w.host.prepare(id.clone(), spec, cwd, args);
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
                    if let Some(tracker) = &mut record.tracker {
                        tracker.on_title(&record.raw_title);
                        record.state.activity = tracker.activity();
                        record.state.attention = tracker.activity().attention();
                    }
                    record.state.title = record.title();
                }
            }
            Err(failure) => {
                record.state.process = Process::Failed;
                record.state.failure = Some(failure.clone());
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
pub fn close(id: &str, confirmed: bool) -> Result<&'static str, String> {
    with(|w| {
        let index = w.index(id)?;
        if w.records[index].state.process != Process::Running {
            w.remove(index, false)?;
            return Ok("closed");
        }
        if w.records[index].closing {
            return Ok("closed");
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
                if reason == CloseReason::ForegroundJob {
                    w.publish_shell_executable(index);
                }
                let before = w.presented();
                w.sheet = Some(Sheet::ConfirmClose {
                    item_id: id.into(),
                    reason,
                    detail: job.and_then(|j| j.name),
                });
                w.present(before);
                return Ok("confirm");
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
        Ok("closed")
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
            Activity::Working => return Some(CloseReason::AgentWorking),
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
        embed::HostEvent::Notify(s) => HostUpdate::Notify(s.into()),
        embed::HostEvent::Bell => HostUpdate::Bell,
    };
    HOST_EVENTS.with(|events| events.borrow_mut().push(key, update));
    if !FLUSH_SCHEDULED.with(|scheduled| scheduled.replace(true)) {
        platform::defer_on_main(Box::new(flush_host_events));
    }
}
fn flush_host_events() {
    FLUSH_SCHEDULED.with(|scheduled| scheduled.set(false));
    let events = HOST_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
    let _ = with(|w| {
        for (key, event) in events.events {
            w.apply_host_event(key, event)?;
        }
        w.refresh_recent();
        Ok(())
    });
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
            r.state.id == item
                && r.tracker.as_ref().and_then(AgentTracker::session_id) == Some(session)
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
        r.state.model = c.model;
        r.state.title = r.title();
        w.changed = true;
        Ok(())
    });
}
pub fn update_model(item: &str, session: &str, model: Option<String>) {
    let _ = with(|w| {
        if let Some(r) = w.records.iter_mut().find(|r| {
            r.state.id == item
                && r.state
                    .conversation
                    .as_ref()
                    .is_some_and(|c| c.id == session)
        }) {
            if r.state.model != model {
                r.state.model = model;
                w.changed = true;
            }
        }
        Ok(())
    });
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
        JumpWaiting => with(|w| {
            // Keep previously visited your-turn entries during a repeated jump, but drop
            // anything that resumed working/exited, and re-sort newly arriving approvals.
            let remembered = if w.waiting_cursor.as_deref() == w.active.as_deref() {
                w.waiting_cycle.as_slice()
            } else {
                &[]
            };
            let order = waiting_cycle_order(w.records.iter().map(|r| &r.state), remembered);
            if let Some(id) = cycle(&order, w.waiting_cursor.as_deref(), true) {
                w.select(&id)?;
                w.waiting_cycle = order;
                w.waiting_cursor = Some(id);
            }
            Ok(())
        }),
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
            self.tracker.as_ref().and_then(AgentTracker::label),
            &self.raw_title,
            &self.state.cwd,
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
        let title_source_changed = if let Some(tracker) = &mut self.tracker {
            tracker.on_title(&state.title)
        } else {
            self.raw_title != state.title
        };
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
    /// OMP status signals; returns whether a new session dropped the bound conversation.
    fn apply_notification(&mut self, payload: &str, discovery_revision: &mut u64) -> bool {
        let Some(tracker) = &mut self.tracker else {
            return false;
        };
        let Some(event) = crate::status::parse_osc777(payload) else {
            return false;
        };
        tracker.on_event(&event);
        let reset = match &event {
            OmpEvent::SessionStart { session_id, .. } => {
                self.start_session(session_id, discovery_revision)
            }
            _ => false,
        };
        if matches!(event, OmpEvent::Stop | OmpEvent::StopFailure { .. }) {
            self.request_model();
        }
        reset
    }
    fn start_session(&mut self, session_id: &str, discovery_revision: &mut u64) -> bool {
        let reset = self
            .state
            .conversation
            .as_ref()
            .is_some_and(|c| c.id != session_id);
        if reset {
            self.state.conversation = None;
            self.state.model = None;
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
    fn request_model(&self) {
        if let Some(c) = &self.state.conversation {
            let _ = runtime::send(Request::Model(
                self.state.id.clone(),
                c.id.clone(),
                c.path.clone(),
            ));
        }
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
    fn derived_activity(&self) -> Activity {
        if self.state.process == Process::Exited
            && self.state.kind == ToolKind::Agent
            && self.state.exit_code != Some(0)
        {
            Activity::Error
        } else {
            self.tracker
                .as_ref()
                .map_or(Activity::Unknown, AgentTracker::activity)
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
pub fn derived_title(
    status: StatusSource,
    omp_label: Option<&str>,
    raw: &str,
    cwd: &str,
    conversation: Option<&str>,
    label: &str,
) -> String {
    let parsed = (status == StatusSource::Omp && omp_label.is_none())
        .then(|| crate::status::parse_omp_title(raw))
        .flatten();
    let title = if status == StatusSource::Omp {
        omp_label
            .or_else(|| parsed.as_ref().map(|(_, label)| label.as_str()))
            .unwrap_or(raw)
            .trim()
    } else {
        raw.trim()
    };
    let basename = Path::new(cwd)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(cwd);
    if title.is_empty() || status == StatusSource::Omp && title == basename {
        conversation
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(if title.is_empty() { label } else { title })
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
    fn apply_host_event(&mut self, key: SessionKey, event: HostUpdate) -> Result<(), String> {
        let Some(index) = self.records.iter().position(|r| r.key == Some(key)) else {
            return Ok(());
        };
        let presented = self.is_presented(index);
        let record = &mut self.records[index];
        let before = record.observed();
        let (title_source_changed, metadata_changed) = match event {
            HostUpdate::State(state) => (
                record.apply_terminal_state(state, &mut self.discovery_revision),
                false,
            ),
            HostUpdate::Notify(payload) => {
                let reset = record.apply_notification(&payload, &mut self.discovery_revision);
                (reset, reset)
            }
            HostUpdate::Bell => (false, false),
        };
        let title_changed = title_source_changed && record.refresh_title();
        let activity = record.derived_activity();
        if activity != record.state.activity || record.state.process != before.0 {
            record.state.state_since = now();
        }
        record.state.activity = activity;
        record.state.attention = activity.attention();
        if activity != before.1 && !presented {
            if activity == Activity::YourTurn {
                record.state.unread = true;
            }
            if let Some(body) = activity_notice(activity, &self.settings) {
                self.host.notify(&record.state.title, body);
            }
        }
        self.changed |= metadata_changed || title_changed || before != record.observed();
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
        self.records.remove(index);
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
