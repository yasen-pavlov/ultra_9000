//! Bounded, read-only document I/O. Directory watches and all file reads belong to a worker.
use base64::{engine::general_purpose::STANDARD, Engine};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs::{self, File, Metadata},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{mpsc, LazyLock, Mutex},
    time::{Duration, Instant, UNIX_EPOCH},
};

pub const MAX_BYTES: u64 = 8 * 1024 * 1024;
const COALESCE: Duration = Duration::from_millis(50);
const MISSING_GRACE: Duration = Duration::from_millis(500);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DocumentStatus {
    Ok,
    Missing,
    Unreadable,
    TooLarge,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentEvent {
    Opened,
    Updated,
    Replaced,
    Missing,
    Returned,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentState {
    pub path: String,
    pub dir: String,
    pub status: DocumentStatus,
    pub error: Option<String>,
    pub revision: u64,
    pub last_event: DocumentEvent,
    pub last_event_at_ms: u64,
    pub modified_ms: u64,
    pub size_bytes: u64,
    pub live_until_ms: u64,
    pub follow: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentText {
    pub revision: u64,
    pub text: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentAsset {
    pub mime: String,
    pub base64: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentEntry {
    pub path: String,
    pub name: String,
    pub dir: String,
    pub modified_ms: u64,
    pub size_bytes: u64,
    pub open_item_id: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentList {
    pub entries: Vec<DocumentEntry>,
    pub truncated: bool,
    pub scanned_at_ms: u64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentPreview {
    pub h1: Option<String>,
    pub heading_count: usize,
    pub first_paragraph: Option<String>,
    pub size_bytes: u64,
    pub modified_ms: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Loaded {
    pub state: DocumentState,
    text: String,
    identity: Option<(u64, u64)>,
}
fn modified(meta: &Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}
pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("md") || s.eq_ignore_ascii_case("markdown"))
}
fn bounded(path: &Path, limit: u64) -> std::io::Result<(Metadata, Vec<u8>)> {
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Document must be a regular file.",
        ));
    }
    if limit == MAX_BYTES && meta.len() > limit {
        return Ok((meta, Vec::new()));
    }
    let mut bytes = Vec::with_capacity(meta.len().min(limit + 1) as usize);
    file.take(limit + 1).read_to_end(&mut bytes)?;
    Ok((meta, bytes))
}
pub fn open(path: &Path, project: &Path, follow: bool, at: u64) -> Result<Loaded, String> {
    let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    if !is_markdown(&path) {
        return Err("Only Markdown documents (.md or .markdown) can be viewed.".into());
    }
    let meta = fs::metadata(&path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Document must be a regular file.".into());
    }
    let parent = path.parent().ok_or("Document has no directory.")?;
    let dir = parent
        .strip_prefix(project)
        .map(|p| {
            if p.as_os_str().is_empty() {
                ".".into()
            } else {
                p.to_string_lossy().into_owned()
            }
        })
        .unwrap_or_else(|_| parent.to_string_lossy().into_owned());
    let mut loaded = Loaded {
        state: DocumentState {
            path: path.to_string_lossy().into_owned(),
            dir,
            status: DocumentStatus::Ok,
            error: None,
            revision: 0,
            last_event: DocumentEvent::Opened,
            last_event_at_ms: at,
            modified_ms: modified(&meta),
            size_bytes: meta.len(),
            live_until_ms: 0,
            follow,
        },
        text: String::new(),
        identity: None,
    };
    loaded.reload(at, true);
    loaded.state.live_until_ms = loaded.state.modified_ms.saturating_add(5000);
    Ok(loaded)
}
impl Loaded {
    fn reload(&mut self, at: u64, initial: bool) {
        let result = bounded(Path::new(&self.state.path), MAX_BYTES);
        self.apply_read(at, initial, result);
    }
    fn apply_read(&mut self, at: u64, initial: bool, result: std::io::Result<(Metadata, Vec<u8>)>) {
        let old_status = self.state.status;
        match result {
            Ok((meta, bytes)) => {
                let identity = (meta.dev(), meta.ino());
                let event = if initial {
                    DocumentEvent::Opened
                } else if old_status == DocumentStatus::Missing {
                    DocumentEvent::Returned
                } else if self.identity.is_some_and(|old| old != identity)
                    || meta.len() < self.state.size_bytes
                {
                    DocumentEvent::Replaced
                } else {
                    DocumentEvent::Updated
                };
                self.identity = Some(identity);
                self.state.size_bytes = meta.len();
                self.state.error = None;
                self.state.live_until_ms = modified(&meta).saturating_add(5000);
                if meta.len() > MAX_BYTES || bytes.len() as u64 > MAX_BYTES {
                    self.state.status = DocumentStatus::TooLarge;
                    self.text.clear();
                } else {
                    let decoded = if bytes.iter().take(8192).any(|byte| *byte == 0) {
                        Err("File contains NUL bytes and is not UTF-8 text.".to_owned())
                    } else {
                        String::from_utf8(bytes).map_err(|error| error.to_string())
                    };
                    match decoded {
                        Ok(text) => {
                            self.text = text;
                            self.state.status = DocumentStatus::Ok;
                            self.state.revision += 1;
                            self.state.modified_ms = modified(&meta);
                        }
                        Err(error) => {
                            self.state.status = DocumentStatus::Unreadable;
                            self.state.error = Some(error.to_string());
                            self.text.clear();
                        }
                    }
                }
                self.state.last_event = event;
            }
            Err(error) => {
                if error.kind() == std::io::ErrorKind::NotFound {
                    self.state.status = DocumentStatus::Missing;
                    self.state.last_event = DocumentEvent::Missing;
                    self.state.error = None;
                } else {
                    self.state.status = DocumentStatus::Unreadable;
                    self.state.error = Some(error.to_string());
                }
                self.text.clear();
            }
        }
        self.state.last_event_at_ms = at;
    }
}
pub fn asset(document: &Path, rel: &str) -> Result<DocumentAsset, String> {
    // Document identity was canonicalized at open. Do not expand a subsequently replaced
    // parent symlink into a new asset authority.
    let root = document.parent().ok_or("Document has no directory.")?;
    let relative = Path::new(rel);
    if relative.is_absolute() {
        return Err("Document images must use relative paths.".into());
    }
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !path.starts_with(root) {
        return Err("Image escapes the document directory.".into());
    }
    let (meta, bytes) = bounded(&path, MAX_BYTES).map_err(|error| error.to_string())?;
    if meta.len() > MAX_BYTES || bytes.len() as u64 > MAX_BYTES {
        return Err("Image exceeds 8 MiB.".into());
    }
    let mime = if let Some(kind) =
        infer::get(&bytes).filter(|kind| kind.mime_type().starts_with("image/"))
    {
        kind.mime_type()
    } else if std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| roxmltree::Document::parse(text).ok())
        .is_some_and(|xml| xml.root_element().tag_name().name() == "svg")
    {
        "image/svg+xml"
    } else {
        return Err("Unsupported image format.".into());
    };
    Ok(DocumentAsset {
        mime: mime.into(),
        base64: STANDARD.encode(bytes),
    })
}
pub fn scan(root: &Path) -> DocumentList {
    scan_bounded(root, 20_000, Duration::from_secs(2))
}
fn scan_bounded(root: &Path, max_entries: usize, budget: Duration) -> DocumentList {
    let started = Instant::now();
    let mut list = DocumentList {
        entries: Vec::with_capacity(51),
        truncated: false,
        scanned_at_ms: crate::workspace::now(),
    };
    for (count, entry) in ignore::WalkBuilder::new(root)
        .hidden(true)
        .require_git(false)
        .follow_links(false)
        .build()
        .enumerate()
    {
        if count >= max_entries || started.elapsed() >= budget {
            list.truncated = true;
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) || !is_markdown(entry.path()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let path = entry.path();
        list.entries.push(DocumentEntry {
            path: path.to_string_lossy().into_owned(),
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            dir: path
                .parent()
                .and_then(|p| p.strip_prefix(root).ok())
                .filter(|p| !p.as_os_str().is_empty())
                .map_or_else(|| ".".into(), |p| p.to_string_lossy().into_owned()),
            modified_ms: modified(&meta),
            size_bytes: meta.len(),
            open_item_id: None,
        });
        list.entries.sort_by(|a, b| {
            b.modified_ms
                .cmp(&a.modified_ms)
                .then_with(|| a.path.cmp(&b.path))
        });
        list.entries.truncate(50);
    }
    list
}

type ProjectCache = HashMap<PathBuf, (u64, DocumentList)>;
static PROJECTS: LazyLock<Mutex<ProjectCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Reuse repeated queries within one presentation; opening the palette advances its epoch.
pub fn project_documents(root: &Path, epoch: u64) -> DocumentList {
    let mut projects = PROJECTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached_epoch, list)) = projects.get(root) {
        if *cached_epoch == epoch {
            return list.clone();
        }
    }
    let list = scan(root);
    projects.insert(root.to_owned(), (epoch, list.clone()));
    list
}

pub fn external_link(url: &str) -> Result<tauri::Url, String> {
    let url = tauri::Url::parse(url).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https" | "mailto") {
        return Err("Only http, https and mailto links can be opened.".into());
    }
    Ok(url)
}
pub fn preview(path: &Path) -> Result<DocumentPreview, String> {
    let (meta, mut bytes) = bounded(path, 64 * 1024).map_err(|error| error.to_string())?;
    bytes.truncate(64 * 1024);
    let text = String::from_utf8_lossy(&bytes);
    let mut preview = DocumentPreview {
        h1: None,
        heading_count: 0,
        first_paragraph: None,
        size_bytes: meta.len(),
        modified_ms: modified(&meta),
    };
    let mut fence = false;
    let mut frontmatter = text.starts_with("---\n") || text.starts_with("---\r\n");
    let mut paragraph = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if frontmatter {
            if index > 0 && matches!(line, "---" | "...") {
                frontmatter = false;
            }
            continue;
        }
        if line.starts_with("```") || line.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        let level = line.bytes().take_while(|b| *b == b'#').count();
        if (1..=6).contains(&level)
            && line
                .as_bytes()
                .get(level)
                .is_some_and(u8::is_ascii_whitespace)
        {
            preview.heading_count += 1;
            if level == 1 && preview.h1.is_none() {
                preview.h1 = Some(line[level..].trim().trim_end_matches('#').trim().into());
            }
        } else if line.is_empty() {
            if !paragraph.is_empty() && preview.first_paragraph.is_none() {
                preview.first_paragraph = Some(paragraph.join(" "));
                paragraph.clear();
            }
        } else if preview.first_paragraph.is_none() && !line.starts_with(['>', '-', '*', '|', '<'])
        {
            paragraph.push(line.to_owned());
        }
    }
    if preview.first_paragraph.is_none() && !paragraph.is_empty() {
        preview.first_paragraph = Some(paragraph.join(" "));
    }
    Ok(preview)
}

struct Watched {
    loaded: Loaded,
    due: Option<Instant>,
    missing_since: Option<Instant>,
    initial: bool,
}
pub struct Manager {
    watcher: RecommendedWatcher,
    unwatched: std::collections::HashSet<PathBuf>,
    directories: HashMap<PathBuf, usize>,
    files: HashMap<String, Watched>,
}
impl Manager {
    pub fn new(
        callback: impl FnMut(Result<Event, notify::Error>) + Send + 'static,
    ) -> Result<Self, String> {
        Ok(Self {
            watcher: notify::recommended_watcher(callback).map_err(|e| e.to_string())?,
            unwatched: std::collections::HashSet::new(),
            directories: HashMap::new(),
            files: HashMap::new(),
        })
    }
    pub fn watch(&mut self, id: String, loaded: Loaded) -> Result<(), String> {
        let dir = Path::new(&loaded.state.path)
            .parent()
            .ok_or("Document has no directory.")?
            .to_owned();
        if !self.directories.contains_key(&dir) {
            self.watcher
                .watch(&dir, RecursiveMode::NonRecursive)
                .map_err(|e| e.to_string())?;
        }
        *self.directories.entry(dir).or_default() += 1;
        self.files.insert(
            id,
            Watched {
                loaded,
                due: Some(Instant::now() + COALESCE),
                missing_since: None,
                initial: true,
            },
        );
        Ok(())
    }
    pub fn forget(&mut self, id: &str) {
        if let Some(file) = self.files.remove(id) {
            let dir = Path::new(&file.loaded.state.path).parent().unwrap();
            if let Some(count) = self.directories.get_mut(dir) {
                *count -= 1;
                if *count == 0 {
                    let _ = self.watcher.unwatch(dir);
                    self.unwatched.remove(dir);
                    self.directories.remove(dir);
                }
            }
        }
    }
    pub fn event(&mut self, event: Event, at: Instant) {
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        if matches!(
            event.kind,
            EventKind::Remove(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
        ) {
            for directory in self.directories.keys() {
                if event.paths.contains(directory) {
                    self.unwatched.insert(directory.clone());
                }
            }
        }
        for file in self.files.values_mut() {
            if event.paths.is_empty()
                || event.paths.iter().any(|p| {
                    p == Path::new(&file.loaded.state.path)
                        || p == Path::new(&file.loaded.state.path).parent().unwrap()
                })
            {
                let due = at + COALESCE;
                if file.missing_since.is_some() {
                    file.due = Some(file.due.map_or(due, |existing| existing.min(due)));
                } else {
                    file.due.get_or_insert(due);
                }
            }
        }
    }
    pub fn timeout(&self, at: Instant) -> Duration {
        self.files
            .values()
            .filter_map(|f| f.due)
            .min()
            .map_or(Duration::from_secs(3600), |d| {
                d.saturating_duration_since(at)
            })
    }
    pub fn due(
        &mut self,
        instant: Instant,
        now: u64,
        mut publish: impl FnMut(String, DocumentState),
    ) {
        let watcher = &mut self.watcher;
        self.unwatched.retain(|directory| {
            !directory.is_dir()
                || watcher
                    .watch(directory, RecursiveMode::NonRecursive)
                    .is_err()
        });
        for (id, file) in &mut self.files {
            if !file.due.is_some_and(|due| due <= instant) {
                continue;
            }
            if Path::new(&file.loaded.state.path)
                .try_exists()
                .is_ok_and(|exists| !exists)
            {
                let since = *file.missing_since.get_or_insert(instant);
                if instant.duration_since(since) < MISSING_GRACE {
                    file.due = Some(since + MISSING_GRACE);
                    continue;
                }
            } else {
                file.missing_since = None;
            }
            file.due = None;
            let directory = Path::new(&file.loaded.state.path).parent().unwrap();
            if !directory.is_dir() {
                self.unwatched.insert(directory.to_owned());
            }
            if self.unwatched.contains(directory) {
                file.due = Some(instant + MISSING_GRACE);
            }
            if file.loaded.state.status == DocumentStatus::Missing
                && Path::new(&file.loaded.state.path)
                    .try_exists()
                    .is_ok_and(|exists| !exists)
            {
                continue;
            }
            let result = bounded(Path::new(&file.loaded.state.path), MAX_BYTES);
            // The path can disappear between the existence probe and open. Preserve the
            // last read here as well: every disappearance gets the full missing grace.
            if result
                .as_ref()
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                let since = *file.missing_since.get_or_insert(instant);
                if instant.duration_since(since) < MISSING_GRACE {
                    file.due = Some(since + MISSING_GRACE);
                    continue;
                }
            }
            file.loaded.apply_read(now, file.initial, result);
            file.initial = false;
            publish(id.clone(), file.loaded.state.clone());
        }
    }
    pub fn text(&self, id: &str) -> Result<DocumentText, String> {
        let file = self.files.get(id).ok_or("Document is closed.")?;
        if file.loaded.state.status != DocumentStatus::Ok {
            return Err("Document text is unavailable.".into());
        }
        Ok(DocumentText {
            revision: file.loaded.state.revision,
            text: file.loaded.text.clone(),
        })
    }
    fn reload(&mut self, id: &str) -> Result<DocumentState, String> {
        let file = self.files.get_mut(id).ok_or("Document is closed.")?;
        file.loaded.reload(crate::workspace::now(), false);
        Ok(file.loaded.state.clone())
    }
}
enum Request {
    Watch(String, Box<Loaded>),
    Forget(String),
    Event(Event),
    Text(String, mpsc::Sender<Result<DocumentText, String>>),
    Reload(String, mpsc::Sender<Result<(), String>>),
}
static SERVICE: LazyLock<mpsc::Sender<Request>> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel();
    let events = tx.clone();
    std::thread::Builder::new()
        .name("u9k-documents".into())
        .spawn(move || {
            let mut manager = match Manager::new(move |event| {
                if let Ok(event) = event {
                    let _ = events.send(Request::Event(event));
                }
            }) {
                Ok(manager) => manager,
                Err(error) => {
                    crate::platform::defer_on_main(Box::new(move || {
                        crate::workspace::action_error(error)
                    }));
                    return;
                }
            };
            loop {
                match rx.recv_timeout(manager.timeout(Instant::now())) {
                    Ok(Request::Watch(id, loaded)) => {
                        if let Err(error) = manager.watch(id, *loaded) {
                            crate::platform::defer_on_main(Box::new(move || {
                                crate::workspace::action_error(error)
                            }));
                        }
                    }
                    Ok(Request::Forget(id)) => manager.forget(&id),
                    Ok(Request::Event(event)) => manager.event(event, Instant::now()),
                    Ok(Request::Text(id, reply)) => {
                        let _ = reply.send(manager.text(&id));
                    }
                    Ok(Request::Reload(id, reply)) => {
                        let result = manager.reload(&id).map(|state| publish(id, state));
                        let _ = reply.send(result);
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                manager.due(Instant::now(), crate::workspace::now(), publish);
            }
        })
        .expect("start document worker");
    tx
});
fn publish(id: String, state: DocumentState) {
    crate::platform::defer_on_main(Box::new(move || {
        crate::workspace::apply_document(&id, state)
    }));
}
pub fn watch(id: String, loaded: Loaded) {
    let _ = SERVICE.send(Request::Watch(id, Box::new(loaded)));
}
pub fn forget(id: String) {
    let _ = SERVICE.send(Request::Forget(id));
}
pub fn text(id: String) -> Result<DocumentText, String> {
    let (tx, rx) = mpsc::channel();
    SERVICE
        .send(Request::Text(id, tx))
        .map_err(|_| "Document worker stopped.")?;
    rx.recv().map_err(|_| "Document worker stopped.")?
}
pub fn reload(id: String) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    SERVICE
        .send(Request::Reload(id, tx))
        .map_err(|_| "Document worker stopped.")?;
    rx.recv().map_err(|_| "Document worker stopped.")?
}
#[cfg(test)]
mod tests;
