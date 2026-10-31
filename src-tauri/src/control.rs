//! Shell control link: a local socket through which a desktop-shell plugin (Noctalia's
//! `muty/ultra9000`, through `ultra9000 ctl`, see `ctl.rs`) mirrors the sidebar and sends the
//! actions the app's own UI offers.
//!
//! The app listens on `control-<pid>.sock` in the OMP link's private runtime directory
//! (mode 0600, per instance). Both directions are newline-delimited JSON, one object of at
//! most [`MAX_LINE`] bytes per line. A client's first line is a `hello`. `watch` clients get a
//! `snapshot` at once and after every change: a projection of the workspace that never carries
//! icons, coalesced so a slow client only ever gets the newest one, never a backlog, plus a
//! `ping` every [`PING_INTERVAL`]. Every command gets one `reply`; commands run on the main
//! thread through the workspace functions the UI uses. Payloads are never logged.
//!
//! With `[tray] show = "auto"` the tray icon is withdrawn while a `watch` client is connected
//! and comes back [`TRAY_HANDOVER`] after the last one left.
use crate::{
    commands::{launch_item, on_main_deferred},
    config::TrayShow,
    lifecycle, omp_link, platform,
    status::{item_status, Activity, Attention},
    workspace::{
        self, CloseOutcome, ItemState, OpenableSheet, PaletteMode, Process, WorkspaceState,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, OnceLock, PoisonError},
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Runtime};

/// Protocol version a client's `hello` must name.
pub const PROTOCOL: u64 = 1;
/// Longest line either side sends, newline excluded (Noctalia's `runStream` line limit).
pub const MAX_LINE: usize = 64 * 1024;
/// A `watch` client hears at least this often, a `ping` when nothing else was sent.
pub const PING_INTERVAL: Duration = Duration::from_secs(5);
/// `[tray] show = "auto"`: the tray icon returns this long after the last watcher left.
pub const TRAY_HANDOVER: Duration = Duration::from_secs(5);
/// Overrides the CLI's socket discovery.
pub const SOCKET_ENV: &str = "ULTRA9000_CONTROL_SOCKET";
/// A connection must say hello within this time.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// A client that stops reading for this long is disconnected.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Titles and job labels are clipped to this many characters (the sidebar elides them anyway).
const MAX_TEXT: usize = 200;
/// Most background jobs listed per item.
const MAX_JOBS: usize = 8;
/// `sun_path` is 104 bytes on macOS (108 on Linux), including the terminating NUL.
const MAX_SOCKET_PATH: usize = 103;
const PING: &str = "{\"type\":\"ping\"}\n";

/// The pid digits of a control socket's file name, `control-<pid>.sock`.
pub(crate) fn socket_pid_digits(name: &str) -> Option<&str> {
    name.strip_prefix("control-")?.strip_suffix(".sock")
}

// ---------------------------------------------------------------------------------------------
// Snapshot projection
// ---------------------------------------------------------------------------------------------

/// What a snapshot reports besides the workspace state itself.
pub struct Context {
    pub window_visible: bool,
    /// The item a jump to what's waiting selects next.
    pub next_waiting: Option<String>,
    pub tray: TrayShow,
    /// Home directory, abbreviated to `~` in project parent paths.
    pub home: Option<PathBuf>,
}

/// The sidebar as a shell plugin renders it. Nothing in it is new state; the plugin infers
/// nothing and renders this.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot<'a> {
    app: AppInfo,
    waiting: Waiting,
    projects: Vec<Project<'a>>,
    items: Vec<Item<'a>>,
    tray: Tray,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: &'static str,
    pid: u32,
    window_visible: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Waiting {
    next_item_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Project<'a> {
    id: &'a str,
    name: &'a str,
    parent_path: String,
    pinned: bool,
    default_tool: &'a str,
    item_ids: Vec<&'a str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Item<'a> {
    id: &'a str,
    project_id: &'a str,
    kind: &'static str,
    tool: &'a str,
    tool_label: &'a str,
    tool_glyph: &'a str,
    title: Cow<'a, str>,
    model: Option<String>,
    state: &'static str,
    attention: Option<Attention>,
    unread: bool,
    selected: bool,
    state_since_ms: u64,
    jobs: Vec<Job<'a>>,
    foreground_job: Option<ForegroundJob<'a>>,
    exit_code: Option<i32>,
    cwd: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Job<'a> {
    kind: &'a str,
    label: Cow<'a, str>,
    started_at_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ForegroundJob<'a> {
    command: &'a str,
    since_ms: u64,
}

#[derive(Debug, Serialize)]
struct Tray {
    show: TrayShow,
}

/// Project the workspace in sidebar order: projects as listed, each project's items in launch
/// order.
pub fn project<'a>(state: &'a WorkspaceState, context: &Context) -> Snapshot<'a> {
    let items: HashMap<&str, &ItemState> = state
        .items
        .iter()
        .filter(|i| i.kind != workspace::ItemKind::Document)
        .map(|i| (i.id.as_str(), i))
        .collect();
    let active = state.active_item_id.as_deref();
    let projects = state
        .projects
        .iter()
        .map(|p| Project {
            id: &p.id,
            name: &p.name,
            parent_path: Path::new(&p.path)
                .parent()
                .map(|parent| tilde(parent, context.home.as_deref()))
                .unwrap_or_default(),
            pinned: p.pinned,
            default_tool: &p.default_tool,
            item_ids: p
                .item_ids
                .iter()
                .filter(|id| items.contains_key(id.as_str()))
                .map(String::as_str)
                .collect(),
        })
        .filter(|p| p.pinned || !p.item_ids.is_empty())
        .collect::<Vec<_>>();
    let items = projects
        .iter()
        .flat_map(|p| p.item_ids.iter())
        .filter_map(|id| items.get(id).copied())
        .filter_map(|item| project_item(state, item, active == Some(item.id.as_str())))
        .collect();
    Snapshot {
        app: AppInfo {
            version: env!("CARGO_PKG_VERSION"),
            pid: std::process::id(),
            window_visible: context.window_visible,
        },
        waiting: Waiting {
            next_item_id: context.next_waiting.clone(),
        },
        projects,
        items,
        tray: Tray { show: context.tray },
    }
}

fn project_item<'a>(
    state: &'a WorkspaceState,
    item: &'a ItemState,
    selected: bool,
) -> Option<Item<'a>> {
    let kind = match item.kind {
        workspace::ItemKind::Agent => "agent",
        workspace::ItemKind::Terminal if item.tool == "neovim" => "neovim",
        workspace::ItemKind::Terminal => "shell",
        workspace::ItemKind::Document => return None,
    };
    let tool = state.tools.iter().find(|t| t.id == item.tool);
    let running = item.process == Some(Process::Running);
    Some(Item {
        id: &item.id,
        project_id: &item.project_id,
        kind,
        tool: &item.tool,
        tool_label: tool.map_or(item.tool.as_str(), |t| t.label.as_str()),
        tool_glyph: tool.map_or("", |t| t.glyph.as_str()),
        title: clip(&item.title),
        model: item.model.as_deref().map(model_short),
        state: item_status(item).name(),
        attention: if running { item.attention } else { None },
        unread: item.unread,
        selected,
        state_since_ms: item.state_since,
        jobs: if running && item.activity == Activity::OnJob {
            item.background_jobs
                .iter()
                .take(MAX_JOBS)
                .map(|job| Job {
                    kind: &job.kind,
                    label: clip(&job.label),
                    started_at_ms: job.started_at_ms,
                })
                .collect()
        } else {
            Vec::new()
        },
        foreground_job: item
            .foreground_job
            .as_ref()
            .filter(|_| running)
            .map(|job| ForegroundJob {
                command: &job.command,
                since_ms: job.since_ms,
            }),
        exit_code: item.exit_code,
        cwd: &item.cwd,
    })
}

/// The sidebar's short model text (`modelShort` in `src/lib/format.ts`): the last path
/// segment without `claude-`, `-4-5` version pairs as `4.5`, other dashes as spaces.
pub fn model_short(model: &str) -> String {
    let name = model.rsplit('/').next().unwrap_or(model);
    let name = name.strip_prefix("claude-").unwrap_or(name);
    let bytes = name.as_bytes();
    let digits = |from: usize| {
        from + bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let mut out = String::with_capacity(name.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'-' {
            let major = digits(i + 1);
            if major > i + 1 && bytes.get(major) == Some(&b'-') {
                let minor = digits(major + 1);
                if minor > major + 1 && matches!(bytes.get(minor), None | Some(b'-')) {
                    out.push_str(&name[copied..i]);
                    out.push(' ');
                    out.push_str(&name[i + 1..major]);
                    out.push('.');
                    out.push_str(&name[major + 1..minor]);
                    copied = minor;
                    i = minor;
                    continue;
                }
            }
        }
        i += 1;
    }
    out.push_str(&name[copied..]);
    out.replace('-', " ")
}

/// `path` with the home directory abbreviated to `~`.
fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.filter(|home| *home != Path::new("/") && !home.as_os_str().is_empty()) {
        Some(home) if path == home => "~".into(),
        Some(home) => match path.strip_prefix(home) {
            Ok(rest) => format!("~/{}", rest.to_string_lossy()),
            Err(_) => path.to_string_lossy().into_owned(),
        },
        None => path.to_string_lossy().into_owned(),
    }
}

fn clip(text: &str) -> Cow<'_, str> {
    match text.char_indices().nth(MAX_TEXT) {
        Some((end, _)) => Cow::Owned(format!("{}…", &text[..end])),
        None => Cow::Borrowed(text),
    }
}

/// The snapshot's JSON without the `type`/`protocol`/`revision` header, fitted under
/// [`MAX_LINE`]: an oversized one first loses its job lists, then its last items (and their
/// projects once empty and unpinned), then its last projects.
fn encode(mut snapshot: Snapshot<'_>) -> String {
    // Room for the header and the newline.
    const LIMIT: usize = MAX_LINE - 96;
    let serialize = |s: &Snapshot<'_>| serde_json::to_string(s).expect("snapshots serialize");
    let mut content = serialize(&snapshot);
    if content.len() <= LIMIT {
        return content;
    }
    let total = (snapshot.items.len(), snapshot.projects.len());
    let jobs = snapshot.items.iter().any(|i| !i.jobs.is_empty());
    if jobs {
        snapshot.items.iter_mut().for_each(|i| i.jobs.clear());
        content = serialize(&snapshot);
    }
    while content.len() > LIMIT {
        // Dropping an item saves about its own JSON, a comma and its quoted id in its
        // project's list, plus that project's JSON when it was the project's last item (items
        // are grouped by project) and the project is not pinned. Drop items until that covers
        // the excess; another round corrects a short estimate or drops projects.
        let mut excess = content.len() - LIMIT;
        let mut dropped = Vec::new();
        while excess > 0 {
            let Some(item) = snapshot.items.pop() else {
                break;
            };
            let size = |json: serde_json::Result<String>| json.map_or(0, |json| json.len() + 1);
            let mut saved = size(serde_json::to_string(&item)) + item.id.len() + 3;
            if snapshot
                .items
                .last()
                .is_none_or(|i| i.project_id != item.project_id)
            {
                saved += snapshot
                    .projects
                    .iter()
                    .find(|p| p.id == item.project_id && !p.pinned)
                    .map_or(0, |p| size(serde_json::to_string(p)));
            }
            excess = excess.saturating_sub(saved);
            dropped.push(item.id);
        }
        if dropped.is_empty() && snapshot.projects.pop().is_none() {
            break;
        }
        snapshot.projects.retain_mut(|project| {
            project.item_ids.retain(|id| !dropped.contains(id));
            project.pinned || !project.item_ids.is_empty()
        });
        content = serialize(&snapshot);
    }
    log::warn!(
        "Shell control link: snapshot too large ({} bytes allowed); left out {}job lists, {} of {} items and {} of {} projects",
        LIMIT,
        if jobs { "" } else { "no " },
        total.0 - snapshot.items.len(),
        total.0,
        total.1 - snapshot.projects.len(),
        total.1,
    );
    content
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

/// A client command (`{"type": "<command>", "id": <number>, ...args}`); unknown fields are
/// ignored.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Command {
    SelectItem {
        item_id: String,
        #[serde(default = "shown")]
        show: bool,
    },
    JumpToWaiting {},
    SetWindowVisible {
        visible: Visibility,
        #[serde(default, deserialize_with = "activation_id")]
        activation_token: Option<String>,
        #[serde(default, deserialize_with = "activation_id")]
        desktop_startup_id: Option<String>,
    },
    NewItem {
        #[serde(default)]
        project_id: Option<String>,
        kind: NewKind,
        #[serde(default)]
        tool: Option<String>,
    },
    OpenDocument {
        path: String,
        #[serde(default)]
        select: bool,
        #[serde(default)]
        follow: bool,
    },
    OpenPalette {
        #[serde(default)]
        project_id: Option<String>,
        #[serde(default)]
        mode: Option<PaletteMode>,
    },
    CloseItem {
        item_id: String,
        #[serde(default)]
        confirmed: bool,
    },
    RequestQuit {},
}

pub(crate) fn valid_activation_id(id: &str) -> bool {
    id.len() <= 4096 && !id.contains('\0')
}

fn activation_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let id = Option::<String>::deserialize(deserializer)?;
    if id.as_deref().is_some_and(|id| !valid_activation_id(id)) {
        return Err(serde::de::Error::custom(
            "activationToken/desktopStartupId must be at most 4096 bytes and contain no NUL",
        ));
    }
    Ok(id)
}

const COMMANDS: &[&str] = &[
    "select_item",
    "jump_to_waiting",
    "set_window_visible",
    "new_item",
    "open_document",
    "open_palette",
    "close_item",
    "request_quit",
];

fn shown() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Visibility {
    Set(bool),
    Toggle(Toggle),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Toggle {
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NewKind {
    Agent,
    Shell,
    Neovim,
}

/// Why a command failed: a stable code and a human sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub error: &'static str,
    pub message: String,
}

impl Failure {
    fn new(error: &'static str, message: impl Into<String>) -> Self {
        Self {
            error,
            message: message.into(),
        }
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::new("failed", message)
    }
}

/// The fields a successful reply adds to `{"type":"reply","id":…,"ok":true}`.
pub type Done = Map<String, Value>;

/// Run `command` as the app's own UI would: main-thread parts through
/// [`on_main_deferred`], launches through [`launch_item`].
pub async fn execute<R: Runtime>(app: AppHandle<R>, command: Command) -> Result<Done, Failure> {
    match command {
        Command::SelectItem { item_id, show } => execute_select_item(app, item_id, show).await,
        Command::JumpToWaiting {} => execute_jump_to_waiting(app).await,
        Command::SetWindowVisible {
            visible,
            activation_token,
            desktop_startup_id,
        } => execute_set_window_visible(app, visible, activation_token, desktop_startup_id).await,
        Command::NewItem {
            project_id,
            kind,
            tool,
        } => execute_new_item(project_id, kind, tool).await,
        Command::OpenDocument {
            path,
            select,
            follow,
        } => {
            if !Path::new(&path).is_absolute() {
                return Err(Failure::new(
                    "invalid_args",
                    "Document path must be absolute.",
                ));
            }
            let item = crate::commands::open_document(path, select, follow).await?;
            Ok(done([("itemId", json!(item))]))
        }
        Command::OpenPalette { project_id, mode } => {
            execute_open_palette(app, project_id, mode).await
        }
        Command::CloseItem { item_id, confirmed } => execute_close_item(item_id, confirmed).await,
        Command::RequestQuit {} => execute_request_quit(app).await,
    }
}

fn unknown_item(id: &str) -> Failure {
    Failure::new("unknown_item", format!("Item {id} does not exist."))
}

async fn execute_select_item<R: Runtime>(
    app: AppHandle<R>,
    item_id: String,
    show: bool,
) -> Result<Done, Failure> {
    on_main(move || {
        if !workspace::has_item(&item_id)? {
            return Err(unknown_item(&item_id));
        }
        if show {
            lifecycle::show_window(&app)?;
        }
        workspace::select(&item_id)?;
        Ok(Done::new())
    })
    .await
}

async fn execute_jump_to_waiting<R: Runtime>(app: AppHandle<R>) -> Result<Done, Failure> {
    on_main(move || {
        let nothing = || Failure::new("nothing_waiting", "Nothing is waiting for you.");
        if workspace::waiting_next()?.is_none() {
            return Err(nothing());
        }
        lifecycle::show_window(&app)?;
        let id = workspace::jump_waiting()?.ok_or_else(nothing)?;
        Ok(done([("itemId", json!(id))]))
    })
    .await
}

async fn execute_set_window_visible<R: Runtime>(
    app: AppHandle<R>,
    visible: Visibility,
    activation_token: Option<String>,
    desktop_startup_id: Option<String>,
) -> Result<Done, Failure> {
    on_main(move || {
        if workspace::is_quitting()? {
            return Err(Failure::new("quitting", "Ultra 9000 is quitting."));
        }
        let visible = match visible {
            Visibility::Set(visible) => visible,
            Visibility::Toggle(Toggle::Toggle) => !lifecycle::window_visible(),
        };
        if visible {
            lifecycle::show_window_with_activation(
                &app,
                activation_token.as_deref(),
                desktop_startup_id.as_deref(),
            )?;
        } else {
            lifecycle::hide_window(&app)?;
        }
        Ok(done([("visible", json!(visible))]))
    })
    .await
}

async fn execute_new_item(
    project_id: Option<String>,
    kind: NewKind,
    tool: Option<String>,
) -> Result<Done, Failure> {
    let (cwd, default) = on_main(move || {
        if let Some(project) = project_id {
            let default = workspace::listed_project_default_tool(&project)?.ok_or_else(|| {
                Failure::new(
                    "unknown_project",
                    format!("Project {project} is not in the sidebar."),
                )
            })?;
            Ok((project, default))
        } else {
            Ok(workspace::default_launch()?)
        }
    })
    .await?;
    let tool = match kind {
        NewKind::Agent => tool.unwrap_or(default),
        NewKind::Shell => "shell".into(),
        NewKind::Neovim => "neovim".into(),
    };
    match launch_item(tool, cwd, None, None).await {
        Ok(id) => Ok(done([("itemId", json!(id))])),
        Err(message) => Err(Failure::new("launch_failed", message)),
    }
}

async fn execute_open_palette<R: Runtime>(
    app: AppHandle<R>,
    project_id: Option<String>,
    mode: Option<PaletteMode>,
) -> Result<Done, Failure> {
    on_main(move || {
        lifecycle::show_window(&app)?;
        workspace::sheet_open(OpenableSheet::Palette {
            project_id,
            mode: mode.unwrap_or(PaletteMode::Open),
            focus: None,
        })?;
        Ok(Done::new())
    })
    .await
}

async fn execute_close_item(item_id: String, confirmed: bool) -> Result<Done, Failure> {
    on_main(move || {
        if !workspace::has_item(&item_id)? {
            return Err(unknown_item(&item_id));
        }
        Ok(match workspace::close_reporting(&item_id, confirmed)? {
            CloseOutcome::Closed => done([("result", json!("closed"))]),
            CloseOutcome::NeedsConfirmation(reason) => done([
                ("result", json!("needs-confirmation")),
                ("reason", json!(reason)),
            ]),
        })
    })
    .await
}

async fn execute_request_quit<R: Runtime>(app: AppHandle<R>) -> Result<Done, Failure> {
    on_main(move || {
        lifecycle::request_quit(&app);
        Ok(Done::new())
    })
    .await
}

/// Run `task` on the main thread.
async fn on_main<T: Send + 'static>(
    task: impl FnOnce() -> Result<T, Failure> + Send + 'static,
) -> Result<T, Failure> {
    on_main_deferred(task).await?
}

fn done<const N: usize>(fields: [(&str, Value); N]) -> Done {
    fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

/// Sends the one reply of a command.
pub struct Responder {
    outbox: Arc<Outbox>,
    id: Value,
}

impl Responder {
    pub fn send(self, result: Result<Done, Failure>) {
        self.outbox.push(reply_line(self.id, result));
    }
}

fn reply_line(id: Value, result: Result<Done, Failure>) -> String {
    let mut reply = Map::new();
    reply.insert("type".into(), json!("reply"));
    reply.insert("id".into(), id);
    match result {
        Ok(fields) => {
            reply.insert("ok".into(), json!(true));
            reply.extend(fields);
        }
        Err(failure) => {
            reply.insert("ok".into(), json!(false));
            reply.insert("error".into(), json!(failure.error));
            reply.insert("message".into(), json!(failure.message));
        }
    }
    let mut line = Value::Object(reply).to_string();
    line.push('\n');
    line
}

/// Parse one command line; a failure is the reply to send instead.
fn parse_command(line: &[u8]) -> Result<(Command, Value), (Value, Failure)> {
    let Ok(Value::Object(request)) = serde_json::from_slice::<Value>(line) else {
        return Err((
            Value::Null,
            Failure::new("bad_request", "Each line must be one JSON object."),
        ));
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if !id.is_number() {
        return Err((
            id,
            Failure::new("bad_request", "A command needs a numeric id."),
        ));
    }
    let kind = request.get("type").and_then(Value::as_str).unwrap_or("");
    if !COMMANDS.contains(&kind) {
        return Err((
            id,
            Failure::new("unknown_command", format!("Unknown command {kind:?}.")),
        ));
    }
    match serde_json::from_value(Value::Object(request)) {
        Ok(command) => Ok((command, id)),
        Err(e) => Err((id, Failure::new("bad_request", e.to_string()))),
    }
}

// ---------------------------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------------------------

/// Output of one connection, written by its writer thread one line at a time. A newer snapshot
/// replaces one not yet written, so a slow client never builds a backlog.
#[derive(Default)]
struct Outbox {
    pending: Mutex<Pending>,
    ready: Condvar,
}

#[derive(Default)]
struct Pending {
    snapshot: Option<Arc<str>>,
    lines: VecDeque<Arc<str>>,
    finished: bool,
}

impl Outbox {
    fn lock(&self) -> MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn offer(&self, snapshot: Arc<str>) {
        let mut pending = self.lock();
        if !pending.finished {
            pending.snapshot = Some(snapshot);
            self.ready.notify_one();
        }
    }

    fn push(&self, line: String) {
        let mut pending = self.lock();
        if !pending.finished {
            pending.lines.push_back(line.into());
            self.ready.notify_one();
        }
    }

    /// No more output is queued; what is queued is still written.
    fn finish(&self) {
        self.lock().finished = true;
        self.ready.notify_one();
    }

    /// Everything queued, the snapshot first, after waiting up to `wait` for something. Empty
    /// after a quiet `wait`; `None` once finished and drained.
    fn take(&self, wait: Duration) -> Option<Vec<Arc<str>>> {
        let pending = self.lock();
        let (mut pending, _) = self
            .ready
            .wait_timeout_while(pending, wait, |p| {
                p.snapshot.is_none() && p.lines.is_empty() && !p.finished
            })
            .unwrap_or_else(PoisonError::into_inner);
        let mut out: Vec<_> = pending.snapshot.take().into_iter().collect();
        out.extend(pending.lines.drain(..));
        (!out.is_empty() || !pending.finished).then_some(out)
    }
}

/// Snapshot fan-out and watcher presence, shared by the connection threads.
#[derive(Default)]
struct Hub(Mutex<HubState>);

#[derive(Default)]
struct HubState {
    /// The newest snapshot's content (without header) and line.
    content: String,
    line: Option<Arc<str>>,
    revision: u64,
    clients: Vec<(Arc<Outbox>, bool)>,
    presence: Presence,
    /// The app is exiting: connections end, new ones are refused.
    closed: bool,
}

/// Connected `watch` clients, for the tray hand-over.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Presence {
    pub watchers: usize,
    /// When the last watcher left.
    pub last_left: Option<Instant>,
}

impl Presence {
    /// Whether the app's own tray icon shows: `auto` withdraws it while a watcher is connected
    /// and until [`TRAY_HANDOVER`] after the last one left.
    pub fn icon_shown(&self, setting: TrayShow, now: Instant) -> bool {
        match setting {
            TrayShow::Always => true,
            TrayShow::Never => false,
            TrayShow::Auto => {
                self.watchers == 0
                    && self
                        .last_left
                        .is_none_or(|left| now.saturating_duration_since(left) >= TRAY_HANDOVER)
            }
        }
    }
}

impl Hub {
    fn lock(&self) -> MutexGuard<'_, HubState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Offer a snapshot (its content without header) to every watcher, unless it is unchanged.
    /// Each distinct snapshot gets the next revision.
    fn publish(&self, content: String) {
        let mut hub = self.lock();
        if hub.line.is_some() && hub.content == content {
            return;
        }
        hub.revision += 1;
        let line: Arc<str> = format!(
            "{{\"type\":\"snapshot\",\"protocol\":{PROTOCOL},\"revision\":{},{}\n",
            hub.revision,
            content.strip_prefix('{').unwrap_or(&content)
        )
        .into();
        hub.content = content;
        for (outbox, watch) in &hub.clients {
            if *watch {
                outbox.offer(line.clone());
            }
        }
        hub.line = Some(line);
    }

    /// Register a client; a watcher is offered the newest snapshot at once (the first one
    /// follows the `presence_changed` refresh).
    fn join(&self, outbox: &Arc<Outbox>, watch: bool) {
        let mut hub = self.lock();
        if hub.closed {
            outbox.finish();
            return;
        }
        hub.clients.push((outbox.clone(), watch));
        if watch {
            hub.presence.watchers += 1;
            if let Some(line) = &hub.line {
                outbox.offer(line.clone());
            }
        }
    }

    /// Unregister a client; returns whether it was the last watcher.
    fn leave(&self, outbox: &Arc<Outbox>) -> bool {
        let mut hub = self.lock();
        let Some(index) = hub.clients.iter().position(|(o, _)| Arc::ptr_eq(o, outbox)) else {
            return false;
        };
        let (_, watch) = hub.clients.swap_remove(index);
        if !watch {
            return false;
        }
        hub.presence.watchers -= 1;
        if hub.presence.watchers > 0 {
            return false;
        }
        hub.presence.last_left = Some(Instant::now());
        true
    }

    fn presence(&self) -> Presence {
        self.lock().presence
    }

    /// Finish every connection.
    fn close(&self) {
        let mut hub = self.lock();
        hub.closed = true;
        hub.presence.watchers = 0;
        for (outbox, _) in hub.clients.drain(..) {
            outbox.finish();
        }
    }
}

type Execute = dyn Fn(Command, Responder) + Send + Sync;
type Notify = dyn Fn() + Send + Sync;

/// A listening control socket.
pub struct Server {
    socket: PathBuf,
    hub: Arc<Hub>,
}

impl Server {
    /// Listen on `control-<pid>.sock` in `dir` (private to this user, see
    /// [`omp_link::private_dir`]). `execute` runs each command and answers through its
    /// [`Responder`]; `presence_changed` is called when a watcher joins or leaves and again
    /// [`TRAY_HANDOVER`] after the last one left (and when a watcher waits for a first snapshot).
    pub fn start(
        dir: &Path,
        execute: impl Fn(Command, Responder) + Send + Sync + 'static,
        presence_changed: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        omp_link::private_dir(dir)?;
        let socket = dir.join(format!("control-{}.sock", std::process::id()));
        if socket.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(format!("socket path {} is too long", socket.display()));
        }
        match fs::remove_file(&socket) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot remove stale {}: {e}", socket.display())),
        }
        let listener = UnixListener::bind(&socket)
            .map_err(|e| format!("cannot listen on {}: {e}", socket.display()))?;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot restrict {}: {e}", socket.display()))?;
        let hub = Arc::new(Hub::default());
        let accepting = hub.clone();
        let execute: Arc<Execute> = Arc::new(execute);
        let presence_changed: Arc<Notify> = Arc::new(presence_changed);
        thread::Builder::new()
            .name("u9k-control".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let hub = accepting.clone();
                    let execute = execute.clone();
                    let presence_changed = presence_changed.clone();
                    if let Err(e) = thread::Builder::new()
                        .name("u9k-control-conn".into())
                        .spawn(move || serve(stream, &hub, &*execute, &presence_changed))
                    {
                        log::warn!("Shell control link: cannot serve a connection: {e}");
                    }
                }
            })
            .map_err(|e| format!("cannot start the listener: {e}"))?;
        Ok(Self { socket, hub })
    }

    /// Send `snapshot` to every watcher if it differs from the last one.
    pub fn publish(&self, snapshot: Snapshot<'_>) {
        self.hub.publish(encode(snapshot));
    }

    /// Publish snapshot content given as a JSON object (tests of the protocol and the CLI).
    #[cfg(test)]
    pub(crate) fn publish_raw(&self, content: &str) {
        self.hub.publish(content.to_owned());
    }

    pub fn presence(&self) -> Presence {
        self.hub.presence()
    }

    /// Remove the socket and end every connection.
    pub fn close(&self) {
        let _ = fs::remove_file(&self.socket);
        self.hub.close();
    }
}

enum Incoming {
    Line,
    TooLong,
    End,
}

/// Read the next line into `line` (without its newline).
fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> Incoming {
    line.clear();
    match reader.take(MAX_LINE as u64 + 1).read_until(b'\n', line) {
        Ok(_) if line.last() == Some(&b'\n') => {
            line.pop();
            Incoming::Line
        }
        Ok(_) if line.len() > MAX_LINE => Incoming::TooLong,
        _ => Incoming::End,
    }
}

#[derive(Deserialize)]
struct Hello {
    #[serde(rename = "type")]
    kind: String,
    protocol: Option<u64>,
    #[serde(default)]
    watch: bool,
}

/// Whether a valid hello asks to watch; `None` for anything but a version-1 hello.
fn parse_hello(line: &[u8]) -> Option<bool> {
    let hello: Hello = serde_json::from_slice(line).ok()?;
    (hello.kind == "hello" && hello.protocol == Some(PROTOCOL)).then_some(hello.watch)
}

/// Serve one connection until it ends.
fn serve(
    mut stream: UnixStream,
    hub: &Arc<Hub>,
    execute: &Execute,
    presence_changed: &Arc<Notify>,
) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut line = Vec::new();
    let _ = stream.set_read_timeout(Some(HELLO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let watch = match read_line(&mut reader, &mut line) {
        Incoming::Line => parse_hello(&line),
        _ => None,
    };
    let Some(watch) = watch else {
        let error =
            format!("{{\"type\":\"error\",\"error\":\"protocol\",\"protocol\":{PROTOCOL}}}\n");
        let _ = stream.write_all(error.as_bytes());
        let _ = stream.shutdown(Shutdown::Both);
        return;
    };
    if stream.set_read_timeout(None).is_err() {
        return;
    }
    let outbox = Arc::new(Outbox::default());
    let writing = outbox.clone();
    if let Err(e) = thread::Builder::new()
        .name("u9k-control-write".into())
        .spawn(move || write_loop(stream, &writing, watch))
    {
        log::warn!("Shell control link: cannot serve a connection: {e}");
        return;
    }
    hub.join(&outbox, watch);
    if watch {
        presence_changed.as_ref()();
    }
    loop {
        match read_line(&mut reader, &mut line) {
            Incoming::Line => match parse_command(&line) {
                Ok((command, id)) => execute(
                    command,
                    Responder {
                        outbox: outbox.clone(),
                        id,
                    },
                ),
                Err((id, failure)) => outbox.push(reply_line(id, Err(failure))),
            },
            Incoming::TooLong => {
                outbox.push(format!(
                    "{{\"type\":\"error\",\"error\":\"line_too_long\",\"message\":\"Lines are limited to {MAX_LINE} bytes.\"}}\n"
                ));
                break;
            }
            Incoming::End => break,
        }
    }
    if hub.leave(&outbox) {
        // The last watcher left: the tray icon returns once the hand-over delay has passed.
        let later = presence_changed.clone();
        let _ = thread::Builder::new()
            .name("u9k-control-tray".into())
            .spawn(move || {
                thread::sleep(TRAY_HANDOVER);
                later.as_ref()();
            });
    }
    if watch {
        presence_changed.as_ref()();
    }
    outbox.finish();
}

/// Write queued output one line at a time; a watcher also gets a ping after each quiet
/// [`PING_INTERVAL`]. A failed write ends the connection.
fn write_loop(mut stream: UnixStream, outbox: &Outbox, watch: bool) {
    let mut last = Instant::now();
    loop {
        let wait = if watch {
            PING_INTERVAL.saturating_sub(last.elapsed())
        } else {
            Duration::from_secs(3600)
        };
        let Some(mut lines) = outbox.take(wait) else {
            break;
        };
        if lines.is_empty() {
            if !watch || last.elapsed() < PING_INTERVAL {
                continue;
            }
            lines.push(Arc::from(PING));
        }
        if lines
            .iter()
            .any(|line| stream.write_all(line.as_bytes()).is_err())
        {
            outbox.finish();
            break;
        }
        last = Instant::now();
    }
    let _ = stream.shutdown(Shutdown::Both);
}

// ---------------------------------------------------------------------------------------------
// The app's link
// ---------------------------------------------------------------------------------------------

static SERVER: OnceLock<Server> = OnceLock::new();
static HOME: LazyLock<Option<PathBuf>> =
    LazyLock::new(|| std::env::var_os("HOME").map(PathBuf::from));

/// Start the app's control link; without it the shell plugin shows the app as not running.
pub fn start(app: AppHandle) {
    let commands = app.clone();
    let server = Server::start(
        &omp_link::runtime_dir(),
        move |command, responder| {
            let app = commands.clone();
            tauri::async_runtime::spawn(async move { responder.send(execute(app, command).await) });
        },
        move || {
            let app = app.clone();
            platform::defer_on_main(Box::new(move || lifecycle::refresh_tray(&app)));
        },
    );
    match server {
        Ok(server) => {
            let _ = SERVER.set(server);
        }
        Err(e) => log::warn!("Shell control link unavailable: {e}"),
    }
}

/// Publish the workspace to watchers (main thread, no workspace borrow held).
pub fn publish(state: &WorkspaceState) {
    let Some(server) = SERVER.get() else {
        return;
    };
    let context = Context {
        window_visible: lifecycle::window_visible(),
        next_waiting: workspace::waiting_next().ok().flatten(),
        tray: workspace::tray_show().unwrap_or_default(),
        home: HOME.clone(),
    };
    server.publish(project(state, &context));
}

/// Whether the app's tray icon shows now under `setting`.
pub fn tray_icon_shown(setting: TrayShow) -> bool {
    let presence = SERVER.get().map(Server::presence).unwrap_or_default();
    presence.icon_shown(setting, Instant::now())
}

/// Remove the socket and end every connection at exit.
pub fn shutdown() {
    if let Some(server) = SERVER.get() {
        server.close();
    }
}

#[cfg(test)]
mod tests;
