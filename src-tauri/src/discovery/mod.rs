//! Read-only discovery: Index::{new,refresh,conversations,errors,omp_by_session_id},
//! project_root, omp_latest_model and store_dirs. Disk calls must run off the UI thread.
//! Conversation is internal metadata; ConversationEntry is the exact wire DTO.
mod claude;
mod codex;
mod metadata;
mod omp;
mod pi;
mod project;
use metadata::Record;
pub use project::project_root;
pub fn omp_latest_model(path: &std::path::Path) -> Option<String> {
    omp::omp_latest_model(path)
}
use crate::config::{StoreKind, ToolSpec};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversation {
    pub tool: String,
    pub id: String,
    pub path: PathBuf,
    pub title: String,
    pub cwd: PathBuf,
    pub cwd_missing: bool,
    pub project_root: PathBuf,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub model: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreError {
    pub tool: String,
    pub path: PathBuf,
    pub message: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationEntry {
    pub tool: String,
    pub id: String,
    pub path: PathBuf,
    pub title: String,
    pub project_id: PathBuf,
    pub cwd: PathBuf,
    pub cwd_missing: bool,
    #[serde(rename = "createdAt")]
    pub created_at_ms: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at_ms: i64,
    pub model: Option<String>,
    pub running_item_id: Option<String>,
    pub maybe_open_elsewhere: bool,
}
impl Conversation {
    pub fn entry(
        &self,
        running_item_id: Option<String>,
        maybe_open_elsewhere: bool,
    ) -> ConversationEntry {
        ConversationEntry {
            tool: self.tool.clone(),
            id: self.id.clone(),
            path: self.path.clone(),
            title: self.title.clone(),
            project_id: self.project_root.clone(),
            cwd: self.cwd.clone(),
            cwd_missing: self.cwd_missing,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            model: self.model.clone(),
            running_item_id,
            maybe_open_elsewhere,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationList {
    pub revision: u64,
    pub conversations: Vec<ConversationEntry>,
    pub errors: Vec<StoreError>,
    pub scanning: bool,
}
pub(super) struct Windows {
    pub prefix: Vec<u8>,
    pub tail: Vec<u8>,
}
pub(super) fn windows(path: &Path, prefix: usize, tail: usize) -> io::Result<Windows> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut first = Vec::with_capacity(prefix);
    (&mut file).take(prefix as u64).read_to_end(&mut first)?;
    if len <= prefix as u64 {
        return Ok(Windows {
            prefix: first,
            tail: Vec::new(),
        });
    }
    let start = len.saturating_sub(tail as u64).max(prefix as u64);
    file.seek(SeekFrom::Start(start.saturating_sub(1)))?;
    let mut last = Vec::with_capacity(tail + 1);
    file.take((tail + 1) as u64).read_to_end(&mut last)?;
    // Include only whole JSONL records; the extra byte disambiguates a line boundary.
    if start > 0 {
        if let Some(n) = last.iter().position(|b| *b == b'\n') {
            last.drain(..=n);
        } else {
            last.clear();
        }
    }
    Ok(Windows {
        prefix: first,
        tail: last,
    })
}
fn records(bytes: &[u8]) -> impl Iterator<Item = Record> + '_ {
    bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(metadata::decode)
}
fn model(r: &Record) -> Option<String> {
    if r.kind == "model_change" && r.role.as_deref().is_none_or(|r| r == "default") {
        r.model.clone().or_else(|| {
            r.model_id
                .as_ref()
                .map(|m| join_model(r.provider.as_deref(), m))
        })
    } else if r.kind == "message"
        && r.message.as_ref().and_then(|m| m.role.as_deref()) == Some("assistant")
    {
        r.message.as_ref().and_then(|m| {
            m.model
                .as_ref()
                .map(|id| join_model(m.provider.as_deref(), id))
        })
    } else {
        None
    }
}
fn join_model(provider: Option<&str>, model: &str) -> String {
    match provider {
        Some(provider) if !model.contains('/') => format!("{provider}/{model}"),
        _ => model.into(),
    }
}
pub(super) fn timestamp(s: Option<&str>) -> i64 {
    s.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}
fn mtime(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(9_007_199_254_740_991) as i64
}
// Each store parser maps its own record shape onto these eight normalised fields; a
// parameter struct would only restate the `Conversation` fields at every call site.
#[allow(clippy::too_many_arguments)]
pub(super) fn conversation(
    tool: &str,
    path: &Path,
    id: String,
    cwd: String,
    title: Option<String>,
    created: i64,
    updated: i64,
    model: Option<String>,
) -> Option<Conversation> {
    let cwd = PathBuf::from(cwd);
    if !cwd.is_absolute() || id.is_empty() {
        return None;
    }
    let title = title.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| {
        let date = chrono::DateTime::from_timestamp_millis(created)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "unknown date".into());
        format!("Untitled · {date}")
    });
    Some(Conversation {
        tool: tool.into(),
        id,
        path: path.to_owned(),
        title,
        cwd_missing: false,
        project_root: cwd.clone(),
        cwd,
        created_at_ms: created,
        updated_at_ms: updated,
        model,
    })
}
fn store_dir(home: &Path, store: StoreKind) -> PathBuf {
    home.join(match store {
        StoreKind::Omp => ".omp/agent/sessions",
        StoreKind::Claude => ".claude/projects",
        StoreKind::Codex => ".codex/sessions",
        StoreKind::Pi => ".pi/agent/sessions",
    })
}
/// Watch recursively. Codex's parent also covers its sibling title index.
pub fn store_dirs(home: &Path, tools: &[ToolSpec]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for t in tools {
        if let Some(s) = t.sessions {
            let d = if s == StoreKind::Codex {
                home.join(".codex")
            } else {
                store_dir(home, s)
            };
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
    }
    dirs
}
/// Canonicalize existing configured roots off-main, preserving unavailable paths.
pub fn normalize_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .map(|root| root.canonicalize().unwrap_or_else(|_| root.clone()))
        .collect()
}
/// Resolve explicit saved references independently of palette discovery filtering.
/// Restrict reads to the configured tool's store, and verify the saved identity.
pub fn read_bound(
    tool: &ToolSpec,
    id: &str,
    path: &Path,
    home: &Path,
    roots: &[PathBuf],
) -> Result<Conversation, String> {
    let store = tool
        .sessions
        .ok_or("This tool has no saved-conversation store.")?;
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Cannot read saved conversation: {e}"))?;
    let dir = store_dir(home, store)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !canonical.starts_with(dir) {
        return Err("The saved conversation is outside this tool's store.".into());
    }
    let updated = fs::metadata(&canonical)
        .and_then(|m| m.modified())
        .map(mtime)
        .map_err(|e| e.to_string())?;
    // Authorize and read the canonical target, but retain the saved/discovery spelling as
    // the conversation identity so symlinked stores still match live resume references.
    let conversation = match store {
        StoreKind::Omp => omp::read_bound(&tool.id, &canonical, updated),
        StoreKind::Claude => claude::read(&tool.id, &canonical, updated),
        StoreKind::Codex => codex::read(&tool.id, &canonical, updated),
        StoreKind::Pi => pi::read(&tool.id, &canonical, updated),
    }
    .map_err(|e| e.to_string())?;
    let mut c = conversation
        .filter(|c| c.id == id)
        .ok_or("The saved conversation was not found or its identity changed.")?;
    c.path = path.to_owned();
    c.cwd_missing = !c.cwd.is_dir();
    let roots = normalize_roots(roots);
    c.project_root = project_root(&c.cwd, &roots);
    if !roots.is_empty() && !roots.iter().any(|root| c.project_root.starts_with(root)) {
        return Err("Directory is outside projects.roots.".into());
    }
    Ok(c)
}
#[derive(Clone)]
struct Cached {
    len: u64,
    modified: SystemTime,
    conversation: Option<Conversation>,
}
pub struct Index {
    home: PathBuf,
    cache: HashMap<(String, PathBuf), Cached>,
    conversations: Vec<Conversation>,
    errors: Vec<StoreError>,
}
impl Index {
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            cache: HashMap::new(),
            conversations: vec![],
            errors: vec![],
        }
    }
    pub fn conversations(&self) -> &[Conversation] {
        &self.conversations
    }
    pub fn errors(&self) -> &[StoreError] {
        &self.errors
    }
    pub fn refresh(&mut self, tools: &[ToolSpec], roots: &[PathBuf]) -> bool {
        let roots = normalize_roots(roots);
        let old_errors = std::mem::take(&mut self.errors);
        let mut old_cache = std::mem::take(&mut self.cache);
        let mut next = HashMap::new();
        let mut conversations = Vec::new();
        let mut projects = HashMap::new();
        for tool in tools {
            let Some(store) = tool.sessions else {
                continue;
            };
            let dir = store_dir(&self.home, store);
            let mut paths = Vec::new();
            collect(
                &dir,
                if store == StoreKind::Codex { 3 } else { 1 },
                &tool.id,
                &mut paths,
                &mut self.errors,
            );
            let titles = self.store_titles(tool, store);
            for path in paths {
                let Some((key, cached)) = self.scan_file(tool, store, path, &mut old_cache) else {
                    continue;
                };
                if let Some(c) = cached
                    .conversation
                    .as_ref()
                    .and_then(|c| listed(c, &titles, &mut projects, &roots))
                {
                    conversations.push(c);
                }
                next.insert(key, cached);
            }
        }
        conversations.sort_by(|a, b| {
            b.updated_at_ms
                .cmp(&a.updated_at_ms)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.tool.cmp(&b.tool))
        });
        self.errors
            .sort_by(|a, b| a.tool.cmp(&b.tool).then_with(|| a.path.cmp(&b.path)));
        let changed = conversations != self.conversations || old_errors != self.errors;
        self.conversations = conversations;
        self.cache = next;
        changed
    }
    /// Codex's sibling title index; other stores carry their titles in the session file.
    fn store_titles(&mut self, tool: &ToolSpec, store: StoreKind) -> BTreeMap<String, String> {
        if store != StoreKind::Codex {
            return BTreeMap::new();
        }
        match codex::titles(&self.home.join(".codex/session_index.jsonl")) {
            Ok(t) => t,
            Err(e) => {
                self.errors.push(StoreError {
                    tool: tool.id.clone(),
                    path: self.home.join(".codex/session_index.jsonl"),
                    message: e.to_string(),
                });
                BTreeMap::new()
            }
        }
    }
    /// Reuses the cached read while length and mtime are unchanged; records failures as errors.
    fn scan_file(
        &mut self,
        tool: &ToolSpec,
        store: StoreKind,
        path: PathBuf,
        old_cache: &mut HashMap<(String, PathBuf), Cached>,
    ) -> Option<((String, PathBuf), Cached)> {
        let (len, modified) = match file_stamp(&path) {
            Ok(stamp) => stamp,
            Err(e) => {
                self.errors.push(StoreError {
                    tool: tool.id.clone(),
                    path,
                    message: e.to_string(),
                });
                return None;
            }
        };
        let key = (tool.id.clone(), path);
        if let Some(c) = old_cache
            .remove(&key)
            .filter(|c| c.len == len && c.modified == modified)
        {
            return Some((key, c));
        }
        match read_store(store, &tool.id, &key.1, mtime(modified)) {
            Ok(conversation) => Some((
                key,
                Cached {
                    len,
                    modified,
                    conversation,
                },
            )),
            Err(e) => {
                self.errors.push(StoreError {
                    tool: tool.id.clone(),
                    path: key.1,
                    message: e.to_string(),
                });
                None
            }
        }
    }
    pub fn omp_by_session_id(&mut self, id: &str) -> Option<Conversation> {
        if let Some(c) = self
            .conversations
            .iter()
            .find(|c| c.id == id && c.path.starts_with(self.home.join(".omp/agent/sessions")))
        {
            return Some(c.clone());
        }
        if id.is_empty() || id.contains(['/', '\\']) {
            return None;
        }
        let mut paths = Vec::new();
        collect(
            &self.home.join(".omp/agent/sessions"),
            1,
            "omp",
            &mut paths,
            &mut self.errors,
        );
        let suffix = format!("_{id}.jsonl");
        for path in paths {
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(&suffix))
            {
                let updated = fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .map(mtime)
                    .unwrap_or(0);
                match omp::read_bound("omp", &path, updated) {
                    Ok(Some(mut c)) if c.id == id => {
                        c.cwd_missing = !c.cwd.is_dir();
                        c.project_root = project_root(&c.cwd, &[]);
                        return Some(c);
                    }
                    Err(e) => self.errors.push(StoreError {
                        tool: "omp".into(),
                        path,
                        message: e.to_string(),
                    }),
                    _ => {}
                }
            }
        }
        None
    }
}
fn file_stamp(path: &Path) -> io::Result<(u64, SystemTime)> {
    let meta = fs::metadata(path)?;
    Ok((meta.len(), meta.modified()?))
}
fn read_store(
    store: StoreKind,
    tool: &str,
    path: &Path,
    updated: i64,
) -> io::Result<Option<Conversation>> {
    match store {
        StoreKind::Omp => omp::read(tool, path, updated),
        StoreKind::Claude => claude::read(tool, path, updated),
        StoreKind::Codex => codex::read(tool, path, updated),
        StoreKind::Pi => pi::read(tool, path, updated),
    }
}
/// The listed copy of a cached conversation: index title, project root and the roots filter.
fn listed(
    cached: &Conversation,
    titles: &BTreeMap<String, String>,
    projects: &mut HashMap<PathBuf, (PathBuf, bool)>,
    roots: &[PathBuf],
) -> Option<Conversation> {
    let mut c = cached.clone();
    if let Some(title) = titles.get(&c.id) {
        c.title = title.clone();
    }
    let (root, missing) = projects
        .entry(c.cwd.clone())
        .or_insert_with(|| (project_root(&c.cwd, roots), !c.cwd.is_dir()));
    c.cwd_missing = *missing;
    c.project_root = root.clone();
    (roots.is_empty() || roots.iter().any(|r| c.project_root.starts_with(r))).then_some(c)
}
fn collect(
    dir: &Path,
    depth: usize,
    tool: &str,
    out: &mut Vec<PathBuf>,
    errors: &mut Vec<StoreError>,
) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            errors.push(StoreError {
                tool: tool.into(),
                path: dir.to_owned(),
                message: e.to_string(),
            });
            return;
        }
    };
    for entry in entries {
        match entry {
            Ok(e) => match e.file_type() {
                Ok(t) if t.is_dir() && depth > 0 => {
                    collect(&e.path(), depth - 1, tool, out, errors)
                }
                Ok(t) if t.is_file() && e.path().extension().is_some_and(|s| s == "jsonl") => {
                    out.push(e.path())
                }
                Ok(_) => {}
                Err(err) => errors.push(StoreError {
                    tool: tool.into(),
                    path: e.path(),
                    message: err.to_string(),
                }),
            },
            Err(e) => errors.push(StoreError {
                tool: tool.into(),
                path: dir.to_owned(),
                message: e.to_string(),
            }),
        }
    }
}
#[cfg(test)]
mod tests;
