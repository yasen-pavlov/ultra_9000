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
    sync::{mpsc, Arc, LazyLock, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const MAX_BYTES: u64 = 8 * 1024 * 1024;
const COALESCE: Duration = Duration::from_millis(50);
const MAX_COALESCE: Duration = Duration::from_millis(250);
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
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
    /// Read or decoding error text, only while unreadable.
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
    pub text: Arc<str>,
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
    text: Arc<str>,
    identity: Option<(u64, u64)>,
    stamp: Option<FileStamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    device: u64,
    inode: u64,
    size: u64,
    modified: Option<SystemTime>,
}
impl FileStamp {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
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
fn bounded(path: &Path, limit: u64, prefix: bool) -> std::io::Result<(Metadata, Vec<u8>)> {
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
    if !prefix && meta.len() > limit {
        return Ok((meta, Vec::new()));
    }
    let read_limit = limit + u64::from(!prefix);
    let mut bytes = Vec::with_capacity(meta.len().min(read_limit) as usize);
    file.take(read_limit).read_to_end(&mut bytes)?;
    Ok((meta, bytes))
}

#[derive(Debug)]
pub enum LocateError {
    OutsideRoots,
    Invalid(String),
}
impl std::fmt::Display for LocateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutsideRoots => formatter.write_str("Document is outside projects.roots."),
            Self::Invalid(error) => formatter.write_str(error),
        }
    }
}
/// Canonical document identity and project, shared by every document entry point.
pub fn locate(path: &Path, roots: &[PathBuf]) -> Result<(PathBuf, PathBuf), LocateError> {
    let path = fs::canonicalize(path).map_err(|error| LocateError::Invalid(error.to_string()))?;
    if !is_markdown(&path) || !path.is_file() {
        return Err(LocateError::Invalid(
            "Expected a regular Markdown file.".into(),
        ));
    }
    let directory = path
        .parent()
        .ok_or_else(|| LocateError::Invalid("Document has no directory.".into()))?;
    let roots = crate::discovery::normalize_roots(roots);
    let project = crate::discovery::project_root(directory, &roots);
    if !roots.is_empty() && !roots.iter().any(|root| project.starts_with(root)) {
        return Err(LocateError::OutsideRoots);
    }
    Ok((path, project))
}
/// Read the canonical document/project pair returned by `locate`.
pub fn open(path: &Path, project: &Path, follow: bool, at: u64) -> Result<Loaded, String> {
    let parent = path.parent().ok_or("Document has no directory.")?;
    let relative = parent
        .strip_prefix(project)
        .map_err(|_| "Document is outside its project.")?;
    let dir = if relative.as_os_str().is_empty() {
        ".".into()
    } else {
        relative.to_string_lossy().into_owned()
    };
    let mut loaded = Loaded {
        state: DocumentState {
            path: path.to_string_lossy().into_owned(),
            dir,
            status: DocumentStatus::Ok,
            error: None,
            revision: 0,
            last_event: DocumentEvent::Opened,
            last_event_at_ms: at,
            modified_ms: 0,
            size_bytes: 0,
            live_until_ms: 0,
            follow,
        },
        text: Arc::from(""),
        identity: None,
        stamp: None,
    };
    loaded.reload(at, true);
    Ok(loaded)
}
impl Loaded {
    fn reload(&mut self, at: u64, strict: bool) {
        let result = bounded(Path::new(&self.state.path), MAX_BYTES, false);
        self.apply_read(at, strict, false, result);
    }
    fn apply_read(
        &mut self,
        at: u64,
        strict: bool,
        force: bool,
        result: std::io::Result<(Metadata, Vec<u8>)>,
    ) -> ReadOutcome {
        match result {
            Ok((meta, bytes)) => {
                let identity = (meta.dev(), meta.ino());
                let stamp = FileStamp::from_metadata(&meta);
                let event = if self.state.status == DocumentStatus::Missing {
                    DocumentEvent::Returned
                } else if self.state.revision == 0 {
                    DocumentEvent::Opened
                } else if self.identity.is_some_and(|old| old != identity)
                    || meta.len() < self.state.size_bytes
                {
                    DocumentEvent::Replaced
                } else {
                    DocumentEvent::Updated
                };
                if meta.len() > MAX_BYTES || bytes.len() as u64 > MAX_BYTES {
                    let changed = self.state.status != DocumentStatus::TooLarge
                        || self.state.size_bytes != meta.len();
                    self.stamp = Some(stamp);
                    if !self.text.is_empty() {
                        self.text = Arc::from("");
                    }
                    if changed {
                        self.state.status = DocumentStatus::TooLarge;
                        self.state.error = None;
                        self.state.size_bytes = meta.len();
                        self.state.live_until_ms = modified(&meta).saturating_add(5000);
                        self.state.last_event = event;
                        self.state.last_event_at_ms = at;
                    }
                    return ReadOutcome::applied(changed);
                }
                let decoded = if bytes.contains(&0) {
                    Err("File contains NUL bytes and is not UTF-8 text.".to_owned())
                } else {
                    match String::from_utf8(bytes) {
                        Ok(text) => Ok(text),
                        Err(error) if !strict && error.utf8_error().error_len().is_none() => {
                            self.stamp = None;
                            return ReadOutcome::Incomplete;
                        }
                        Err(error) => Err(error.to_string()),
                    }
                };
                self.stamp = Some(stamp);
                match decoded {
                    Ok(text) => {
                        let unchanged_text = self.text.as_ref() == text;
                        let same = !force
                            && self.state.revision > 0
                            && self.state.status != DocumentStatus::TooLarge
                            && unchanged_text;
                        let status_changed =
                            self.state.status != DocumentStatus::Ok || self.state.error.is_some();
                        let modified = modified(&meta);
                        let live_until = modified.saturating_add(5000);
                        let changed = !same
                            || status_changed
                            || self.state.modified_ms != modified
                            || self.state.size_bytes != meta.len()
                            || self.state.live_until_ms != live_until;
                        if !same {
                            if !unchanged_text {
                                self.text = Arc::from(text);
                            }
                            self.state.revision += 1;
                        }
                        if !same || status_changed {
                            self.state.last_event = event;
                            self.state.last_event_at_ms = at;
                        }
                        self.identity = Some(identity);
                        self.state.status = DocumentStatus::Ok;
                        self.state.error = None;
                        self.state.modified_ms = modified;
                        self.state.size_bytes = meta.len();
                        self.state.live_until_ms = live_until;
                        ReadOutcome::applied(changed)
                    }
                    Err(error) => {
                        let changed = self.state.status != DocumentStatus::Unreadable
                            || self.state.error.as_deref() != Some(error.as_str())
                            || self.state.size_bytes != meta.len();
                        if changed {
                            self.state.status = DocumentStatus::Unreadable;
                            self.state.error = Some(error);
                            self.state.size_bytes = meta.len();
                            self.state.last_event = event;
                            self.state.last_event_at_ms = at;
                        }
                        ReadOutcome::applied(changed)
                    }
                }
            }
            Err(error) => {
                self.stamp = None;
                let (status, error) = if error.kind() == std::io::ErrorKind::NotFound {
                    (DocumentStatus::Missing, None)
                } else {
                    (DocumentStatus::Unreadable, Some(error.to_string()))
                };
                let changed = self.state.status != status || self.state.error != error;
                if changed {
                    self.state.status = status;
                    self.state.error = error;
                    if status == DocumentStatus::Missing {
                        self.state.last_event = DocumentEvent::Missing;
                    }
                    self.state.last_event_at_ms = at;
                }
                ReadOutcome::applied(changed)
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReadOutcome {
    Changed,
    Unchanged,
    Incomplete,
}
impl ReadOutcome {
    fn applied(changed: bool) -> Self {
        if changed {
            Self::Changed
        } else {
            Self::Unchanged
        }
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
    let (meta, bytes) = bounded(&path, MAX_BYTES, false).map_err(|error| error.to_string())?;
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
        entries: Vec::new(),
        truncated: false,
        scanned_at_ms: crate::workspace::now(),
    };
    for (count, entry) in ignore::WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_some_and(|kind| kind.is_dir())
                || !entry.file_name().to_string_lossy().starts_with('.')
        })
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
    }
    list.entries.sort_by(|a, b| {
        b.modified_ms
            .cmp(&a.modified_ms)
            .then_with(|| a.path.cmp(&b.path))
    });
    list.entries.truncate(50);
    list
}

type ProjectCache = HashMap<PathBuf, (u64, DocumentList)>;
static PROJECTS: LazyLock<Mutex<ProjectCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Reuse repeated queries within one presentation; opening the palette advances its epoch.
pub fn project_documents(root: &Path, epoch: u64) -> DocumentList {
    project_documents_with(root, epoch, scan)
}
fn project_documents_with(
    root: &Path,
    epoch: u64,
    scan: impl FnOnce(&Path) -> DocumentList,
) -> DocumentList {
    {
        let projects = PROJECTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((cached_epoch, list)) = projects.get(root) {
            if *cached_epoch == epoch {
                return list.clone();
            }
        }
    }
    let list = scan(root);
    PROJECTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(root.to_owned(), (epoch, list.clone()));
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
    let (meta, bytes) = bounded(path, 64 * 1024, true).map_err(|error| error.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let mut preview = DocumentPreview {
        h1: None,
        heading_count: 0,
        first_paragraph: None,
        size_bytes: meta.len(),
        modified_ms: modified(&meta),
    };
    let mut fence: Option<&str> = None;
    let mut frontmatter = text.starts_with("---\n") || text.starts_with("---\r\n");
    let mut paragraph: Vec<&str> = Vec::new();
    let flush_paragraph = |paragraph: &mut Vec<&str>, preview: &mut DocumentPreview| {
        if !paragraph.is_empty() && preview.first_paragraph.is_none() {
            preview.first_paragraph = Some(paragraph.join(" "));
        }
        paragraph.clear();
    };
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if frontmatter {
            if index > 0 && matches!(line, "---" | "...") {
                frontmatter = false;
            }
            continue;
        }
        if let Some(marker) = fence {
            if line.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if line.starts_with("```") || line.starts_with("~~~") {
            flush_paragraph(&mut paragraph, &mut preview);
            fence = Some(&line[..3]);
            continue;
        }
        let setext = !paragraph.is_empty()
            && !line.is_empty()
            && (line.bytes().all(|byte| byte == b'=') || line.bytes().all(|byte| byte == b'-'));
        if setext {
            preview.heading_count += 1;
            if line.starts_with('=') && preview.h1.is_none() {
                preview.h1 = Some(paragraph.join(" "));
            }
            paragraph.clear();
            continue;
        }
        let level = line.bytes().take_while(|byte| *byte == b'#').count();
        if (1..=6).contains(&level)
            && (level == line.len()
                || line
                    .as_bytes()
                    .get(level)
                    .is_some_and(u8::is_ascii_whitespace))
        {
            flush_paragraph(&mut paragraph, &mut preview);
            preview.heading_count += 1;
            if level == 1 && preview.h1.is_none() {
                let heading = line[level..].trim();
                let closing = heading.trim_end_matches('#');
                let heading = if closing.ends_with(char::is_whitespace) {
                    closing.trim_end()
                } else {
                    heading
                };
                preview.h1 = Some(heading.into());
            }
        } else if line.is_empty() || line.starts_with(['>', '-', '*', '|', '<']) {
            flush_paragraph(&mut paragraph, &mut preview);
        } else {
            paragraph.push(line);
        }
    }
    flush_paragraph(&mut paragraph, &mut preview);
    Ok(preview)
}

struct Watched {
    loaded: Loaded,
    due: Option<Instant>,
    missing_since: Option<Instant>,
    first_event: Option<Instant>,
}
pub struct Manager {
    watcher: Option<RecommendedWatcher>,
    unwatched: std::collections::HashSet<PathBuf>,
    directories: HashMap<PathBuf, usize>,
    files: HashMap<String, Watched>,
}
impl Manager {
    pub fn new(
        callback: impl FnMut(Result<Event, notify::Error>) + Send + 'static,
    ) -> Result<Self, String> {
        Ok(Self::from_watcher(Some(
            notify::recommended_watcher(callback).map_err(|error| error.to_string())?,
        )))
    }
    fn from_watcher(watcher: Option<RecommendedWatcher>) -> Self {
        Self {
            watcher,
            unwatched: std::collections::HashSet::new(),
            directories: HashMap::new(),
            files: HashMap::new(),
        }
    }
    pub fn watch(&mut self, id: String, loaded: Loaded) -> Result<(), String> {
        let dir = Path::new(&loaded.state.path)
            .parent()
            .ok_or("Document has no directory.")?
            .to_owned();
        let watch_error = if !self.directories.contains_key(&dir) {
            match &mut self.watcher {
                Some(watcher) => watcher
                    .watch(&dir, RecursiveMode::NonRecursive)
                    .err()
                    .map(|error| error.to_string()),
                None => None,
            }
        } else {
            None
        };
        if self.watcher.is_none() || watch_error.is_some() {
            self.unwatched.insert(dir.clone());
        }
        *self.directories.entry(dir).or_default() += 1;
        self.files.insert(
            id,
            Watched {
                loaded,
                due: Some(Instant::now() + COALESCE),
                missing_since: None,
                first_event: None,
            },
        );
        watch_error.map_or(Ok(()), Err)
    }
    pub fn forget(&mut self, id: &str) {
        if let Some(file) = self.files.remove(id) {
            let dir = Path::new(&file.loaded.state.path).parent().unwrap();
            if let Some(count) = self.directories.get_mut(dir) {
                *count -= 1;
                if *count == 0 {
                    if let Some(watcher) = &mut self.watcher {
                        let _ = watcher.unwatch(dir);
                    }
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
                let first = *file.first_event.get_or_insert(at);
                file.due = Some((at + COALESCE).min(first + MAX_COALESCE));
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
    pub fn due(&mut self, instant: Instant, now: u64, publish: impl FnMut(String, DocumentState)) {
        self.due_with(instant, now, publish, |path| {
            bounded(path, MAX_BYTES, false)
        });
    }
    fn due_with(
        &mut self,
        instant: Instant,
        now: u64,
        mut publish: impl FnMut(String, DocumentState),
        mut read: impl FnMut(&Path) -> std::io::Result<(Metadata, Vec<u8>)>,
    ) {
        let watcher = &mut self.watcher;
        self.unwatched.retain(|directory| {
            !directory.is_dir()
                || watcher.as_mut().is_none_or(|watcher| {
                    watcher
                        .watch(directory, RecursiveMode::NonRecursive)
                        .is_err()
                })
        });
        for (id, file) in &mut self.files {
            if !file.due.is_some_and(|due| due <= instant) {
                continue;
            }
            file.due = None;
            let event_driven = file.first_event.take().is_some();
            let directory = Path::new(&file.loaded.state.path).parent().unwrap();
            if !directory.is_dir() {
                self.unwatched.insert(directory.to_owned());
            }
            let polling = self.unwatched.contains(directory);
            if polling {
                file.due = Some(instant + MISSING_GRACE);
            }
            let path = Path::new(&file.loaded.state.path);
            if polling
                && !event_driven
                && fs::metadata(path).is_ok_and(|metadata| {
                    metadata.is_file()
                        && file.loaded.stamp == Some(FileStamp::from_metadata(&metadata))
                })
            {
                continue;
            }
            let result = read(path);
            if result
                .as_ref()
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                let since = *file.missing_since.get_or_insert(instant);
                if instant.duration_since(since) < MISSING_GRACE {
                    file.due = Some(since + MISSING_GRACE);
                    continue;
                }
                if file.loaded.state.status == DocumentStatus::Missing {
                    continue;
                }
            } else {
                file.missing_since = None;
            }
            match file.loaded.apply_read(now, !event_driven, false, result) {
                ReadOutcome::Incomplete => file.due = Some(instant + COALESCE),
                ReadOutcome::Changed => publish(id.clone(), file.loaded.state.clone()),
                ReadOutcome::Unchanged => {}
            }
        }
    }
    pub fn text(&self, id: &str) -> Result<DocumentText, String> {
        let file = self.files.get(id).ok_or("Document is closed.")?;
        if file.loaded.state.revision == 0 || file.loaded.state.status == DocumentStatus::TooLarge {
            return Err("Document text is unavailable.".into());
        }
        Ok(DocumentText {
            revision: file.loaded.state.revision,
            text: file.loaded.text.clone(),
        })
    }
    fn reload(&mut self, id: &str) -> Result<DocumentState, String> {
        let file = self.files.get_mut(id).ok_or("Document is closed.")?;
        file.first_event = None;
        file.missing_since = None;
        file.due = self
            .unwatched
            .contains(Path::new(&file.loaded.state.path).parent().unwrap())
            .then(|| Instant::now() + MISSING_GRACE);
        let result = bounded(Path::new(&file.loaded.state.path), MAX_BYTES, false);
        file.loaded
            .apply_read(crate::workspace::now(), true, true, result);
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
static SERVICE: LazyLock<Result<mpsc::Sender<Request>, String>> = LazyLock::new(|| {
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
                    report_error(error);
                    Manager::from_watcher(None)
                }
            };
            loop {
                match rx.recv_timeout(manager.timeout(Instant::now())) {
                    Ok(Request::Watch(id, loaded)) => {
                        if let Err(error) = manager.watch(id, *loaded) {
                            report_error(error);
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
        .map_err(|error| format!("Could not start document worker: {error}"))?;
    Ok(tx)
});
fn report_error(error: String) {
    crate::platform::defer_on_main(Box::new(move || crate::workspace::action_error(error)));
}
fn publish(id: String, state: DocumentState) {
    crate::platform::defer_on_main(Box::new(move || {
        crate::workspace::apply_document(&id, state)
    }));
}
pub fn watch(id: String, loaded: Loaded) {
    let result = SERVICE.as_ref().map_err(Clone::clone).and_then(|service| {
        service
            .send(Request::Watch(id, Box::new(loaded)))
            .map_err(|_| "Document worker stopped.".to_owned())
    });
    if let Err(error) = result {
        report_error(error);
    }
}
pub fn forget(id: String) {
    if let Ok(service) = SERVICE.as_ref() {
        let _ = service.send(Request::Forget(id));
    }
}
pub fn text(id: String) -> Result<DocumentText, String> {
    let (tx, rx) = mpsc::channel();
    SERVICE
        .as_ref()
        .map_err(Clone::clone)?
        .send(Request::Text(id, tx))
        .map_err(|_| "Document worker stopped.")?;
    wait_reply(rx, REPLY_TIMEOUT)
}
pub fn reload(id: String) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    SERVICE
        .as_ref()
        .map_err(Clone::clone)?
        .send(Request::Reload(id, tx))
        .map_err(|_| "Document worker stopped.")?;
    wait_reply(rx, REPLY_TIMEOUT)
}
fn wait_reply<T>(reply: mpsc::Receiver<Result<T, String>>, timeout: Duration) -> Result<T, String> {
    reply.recv_timeout(timeout).map_err(|error| match error {
        mpsc::RecvTimeoutError::Timeout => "Document worker is busy.",
        mpsc::RecvTimeoutError::Disconnected => "Document worker stopped.",
    })?
}
#[cfg(test)]
mod tests;
