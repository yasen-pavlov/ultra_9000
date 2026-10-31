//! ConfigStore::{load,reload,settings,snapshot,set,reset}; all disk I/O is blocking.
use super::{
    paths::ConfigPaths,
    schema::{
        defaults, key_name, validate, IconTint, KeepAwake, MarkdownRoute, SettingValue, Settings,
        SettingsError, SettingsFile, SettingsState, Source, StatusSource, StoreKind, ToolIcon,
        ToolKind, ToolSpec, TrayShow, WriteTarget, UI_KEYS,
    },
    write,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
type Flat = BTreeMap<Vec<String>, toml::Value>;
#[derive(Clone, Default)]
struct FileState {
    text: String,
    values: Flat,
    sources: BTreeMap<Vec<String>, Source>,
}
/// How a config file's keys map onto settings and which of them are validated.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FileKind {
    /// `ultra.toml` and machine files: every key is a validated setting.
    Settings,
    /// `terminal.toml`: Alacritty syntax; only `font.size` is a (validated) setting.
    Terminal,
    /// `keys.toml`: kept as a raw table for the shortcut layer.
    Keys,
}
const FONT_SIZE: [&str; 3] = ["terminal", "font", "size"];
fn font_size_key() -> Vec<String> {
    FONT_SIZE.map(String::from).into()
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Reloaded {
    pub settings: bool,
    pub terminal: bool,
    pub keys: bool,
}
pub struct ConfigStore {
    pub paths: ConfigPaths,
    files: BTreeMap<PathBuf, FileState>,
    settings: Settings,
    values: Flat,
    sources: BTreeMap<Vec<String>, Source>,
    keys: Option<toml::Table>,
    errors: Vec<SettingsError>,
    reported: Vec<SettingsError>,
    revision: u64,
    reloaded_at: u64,
}
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(9_007_199_254_740_991) as u64
}
fn flatten(table: &toml::Table, prefix: &mut Vec<String>, out: &mut Flat) {
    for (k, v) in table {
        prefix.push(k.clone());
        if let Some(t) = v.as_table() {
            flatten(t, prefix, out)
        } else {
            out.insert(prefix.clone(), v.clone());
        }
        prefix.pop();
    }
}
fn line_for(doc: Option<&toml_edit::Document<&str>>, newlines: &[usize], path: &[String]) -> usize {
    let Some(doc) = doc else { return 1 };
    let mut item = doc.as_item();
    for p in path {
        let Some(next) = item.get(p) else { return 1 };
        item = next;
    }
    item.span().map_or(1, |span| {
        newlines.partition_point(|offset| *offset < span.start) + 1
    })
}
fn error(
    file: &Path,
    line: Option<usize>,
    key: Option<String>,
    message: String,
    last_good: Option<String>,
) -> SettingsError {
    SettingsError {
        file: file.to_string_lossy().into(),
        line,
        key,
        message,
        last_good,
    }
}
impl ConfigStore {
    pub fn load(paths: ConfigPaths) -> Self {
        let mut s = Self {
            paths,
            files: BTreeMap::new(),
            settings: Settings::default(),
            values: defaults(),
            sources: BTreeMap::new(),
            keys: None,
            errors: vec![],
            reported: vec![],
            revision: 0,
            reloaded_at: 0,
        };
        s.reload();
        s
    }
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
    pub fn terminal_toml(&self) -> &str {
        self.files
            .get(&self.paths.terminal)
            .map(|f| f.text.as_str())
            .unwrap_or("")
    }
    pub fn keys_toml(&self) -> Option<&toml::Table> {
        self.keys_table()
    }
    fn keys_table(&self) -> Option<&toml::Table> {
        self.keys.as_ref()
    }
    pub fn report_error(&mut self, err: SettingsError) {
        if !self.reported.contains(&err) {
            self.reported.push(err.clone());
            self.errors.push(err);
            self.revision += 1;
        }
    }
    fn read_file(&mut self, path: &Path, kind: FileKind) {
        let Some((text, table)) = self.read_table(path) else {
            return;
        };
        if let Some((values, sources)) = self.accept_values(path, kind, &text, &table) {
            self.files.insert(
                path.to_owned(),
                FileState {
                    text,
                    values,
                    sources,
                },
            );
        }
    }
    /// Reads and parses one file. A missing file is forgotten; an unreadable or
    /// unparseable one is reported and keeps its last good state.
    fn read_table(&mut self, path: &Path) -> Option<(String, toml::Table)> {
        let text = match write::read_bounded(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.files.remove(path);
                return None;
            }
            Err(e) => {
                self.errors
                    .push(error(path, None, None, e.to_string(), None));
                return None;
            }
        };
        match text.parse::<toml::Table>() {
            Ok(table) => Some((text, table)),
            Err(e) => {
                let line = e
                    .span()
                    .map(|r| text[..r.start].bytes().filter(|b| *b == b'\n').count() + 1);
                self.errors.push(error(
                    path,
                    line,
                    None,
                    "unparseable TOML; retaining last good file".into(),
                    None,
                ));
                None
            }
        }
    }
    /// Validates a parsed file key by key. An invalid key keeps this file's previous
    /// value; an invalid terminal setting rejects the whole terminal file (`None`).
    fn accept_values(
        &mut self,
        path: &Path,
        kind: FileKind,
        text: &str,
        table: &toml::Table,
    ) -> Option<(Flat, BTreeMap<Vec<String>, Source>)> {
        // One source parse per document, rather than copying/reparsing for every setting.
        let source_doc = toml_edit::Document::parse(text).ok();
        let newlines: Vec<_> = text
            .bytes()
            .enumerate()
            .filter_map(|(i, b)| (b == b'\n').then_some(i))
            .collect();
        let mut flat = Flat::new();
        flatten(table, &mut vec![], &mut flat);
        let previous = self.files.get(path).cloned().unwrap_or_default();
        let mut accepted = Flat::new();
        let mut sources = BTreeMap::new();
        for (source_key, value) in flat {
            let key = setting_key(kind, &source_key);
            let validation = if is_validated(kind, &key) {
                validate(&key, &value)
            } else {
                Ok(())
            };
            let line = line_for(source_doc.as_ref(), &newlines, &source_key);
            let Err(message) = validation else {
                let file = path.to_string_lossy().into();
                sources.insert(key.clone(), Source { file, line });
                accepted.insert(key, value);
                continue;
            };
            let last = previous.values.get(&key).or_else(|| self.values.get(&key));
            self.errors.push(error(
                path,
                Some(line),
                Some(key_name(&key)),
                message,
                last.map(ToString::to_string),
            ));
            // Do not forward an invalid terminal setting to the renderer.
            if kind == FileKind::Terminal {
                return None;
            }
            retain_previous(&previous, key, &mut accepted, &mut sources);
        }
        Some((accepted, sources))
    }
    fn read_files(&mut self) {
        for (p, kind) in [
            (self.paths.ultra.clone(), FileKind::Settings),
            (self.paths.machine_file.clone(), FileKind::Settings),
            (self.paths.terminal.clone(), FileKind::Terminal),
            (self.paths.keys.clone(), FileKind::Keys),
        ] {
            self.read_file(&p, kind);
        }
    }
    /// Layers built-in defaults, terminal.toml's font size, ultra.toml and the machine file.
    fn merge_layers(&mut self) {
        self.values = defaults();
        self.sources.clear();
        let font = font_size_key();
        if let Some(f) = self.files.get(&self.paths.terminal) {
            if let Some(v) = f.values.get(&font) {
                self.values.insert(font.clone(), v.clone());
                if let Some(source) = f.sources.get(&font) {
                    self.sources.insert(font, source.clone());
                }
            }
        }
        for p in [&self.paths.ultra, &self.paths.machine_file] {
            if let Some(f) = self.files.get(p) {
                self.values.extend(f.values.clone());
                self.sources.extend(f.sources.clone());
            }
        }
    }
    /// Re-reads user images on every reload, including a file-watch-only reload.
    /// Invalid image data never retains a stale user logo: the UI falls back.
    fn load_icons(&mut self, s: &mut Settings) {
        let mut loaded = BTreeMap::<String, Option<String>>::new();
        for (p, value) in self.values.iter().filter(|(p, _)| is_icon_key(p)) {
            let relative = value.as_str().unwrap();
            if !loaded.contains_key(relative) {
                let url = match self.load_icon(p, relative) {
                    Ok(url) => Some(url),
                    Err(e) => {
                        self.errors.push(e);
                        None
                    }
                };
                loaded.insert(relative.to_owned(), url);
            }
            if let Some(url) = &loaded[relative] {
                apply_icon(s, &self.values, p, url);
            }
        }
    }
    /// Loads one user image; a failure points at the setting's source, not the image.
    fn load_icon(&self, p: &[String], relative: &str) -> Result<String, SettingsError> {
        super::icons::load(&self.paths.dir, relative).map_err(|message| {
            let source = self.sources.get(p);
            error(
                source
                    .map(|s| Path::new(&s.file))
                    .unwrap_or(&self.paths.ultra),
                source.map(|s| s.line),
                Some(key_name(p)),
                format!(
                    "{}: {message}; using the built-in mark or configured glyph",
                    self.paths.dir.join(relative).display()
                ),
                None,
            )
        })
    }
    /// A default referring to a removed/unknown tool never escapes to launch callers.
    fn repair_default_tools(&mut self, s: &mut Settings, before: &Settings) {
        let invalid: Vec<_> = self
            .values
            .iter()
            .filter(|(p, v)| {
                p.last().is_some_and(|k| k == "default_tool")
                    && s.tool(v.as_str().unwrap()).is_none()
            })
            .map(|(p, _)| p.clone())
            .collect();
        for p in invalid {
            let fallback = fallback_tool(s, before, &p);
            let source = self.sources.get(&p);
            self.errors.push(SettingsError {
                file: source.map(|s| s.file.clone()).unwrap_or_default(),
                line: source.map(|s| s.line),
                key: Some(key_name(&p)),
                message: "expected a configured tool id".into(),
                last_good: Some(fallback.clone()),
            });
            self.values
                .insert(p.clone(), toml::Value::String(fallback.clone()));
            if p[0] == "launch" {
                s.default_tool = fallback;
            } else {
                s.projects.get_mut(Path::new(&p[1])).unwrap().default_tool = Some(fallback);
            }
        }
    }
    pub fn reload(&mut self) -> Reloaded {
        let before = self.settings.clone();
        let terminal_before = self.terminal_toml().to_owned();
        let keys_before = self.keys.clone();
        self.errors.clear();
        self.reported.clear();
        self.read_files();
        self.merge_layers();
        self.keys = self
            .files
            .get(&self.paths.keys)
            .and_then(|f| f.text.parse().ok());
        let mut s = Settings::default();
        for (p, v) in self.values.iter().filter(|(p, _)| is_tool_key(p)) {
            apply_tool_field(tool_entry(&mut s, &p[1]), &p[2..], v);
        }
        for (p, v) in &self.values {
            apply_setting(&mut s, p, v, self.sources.contains_key(p));
        }
        self.load_icons(&mut s);
        self.repair_default_tools(&mut s, &before);
        self.settings = s;
        self.revision += 1;
        self.reloaded_at = now();
        Reloaded {
            settings: self.settings != before,
            terminal: self.terminal_toml() != terminal_before
                || self.settings.terminal_font_size != before.terminal_font_size,
            keys: self.keys != keys_before,
        }
    }
    pub fn snapshot(&self) -> SettingsState {
        let default = defaults();
        let mut values = Vec::new();
        for (key, value) in &self.values {
            let name = key_name(key);
            if UI_KEYS.contains(&name.as_str()) || is_project_default(key) {
                values.push(SettingValue {
                    key: name,
                    value: serde_json::to_value(value).unwrap(),
                    source: self.sources.get(key).cloned(),
                    default_value: default
                        .get(key)
                        .map(|v| serde_json::to_value(v).unwrap())
                        .unwrap_or_else(|| serde_json::json!(self.settings.default_tool)),
                });
            }
        }
        SettingsState {
            revision: self.revision,
            config_dir: self.paths.dir.to_string_lossy().into(),
            machine: self.paths.machine.clone(),
            files: [
                &self.paths.ultra,
                &self.paths.terminal,
                &self.paths.keys,
                &self.paths.machine_file,
            ]
            .into_iter()
            .map(|p| SettingsFile {
                name: p
                    .strip_prefix(&self.paths.dir)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .into(),
                path: p.to_string_lossy().into(),
                exists: p.exists(),
            })
            .collect(),
            values,
            errors: self.errors.clone(),
            reloaded_at: self.reloaded_at,
        }
    }
    fn target(&self, key: &str, target: WriteTarget) -> Result<(PathBuf, Vec<String>), String> {
        let p = parse_key(key)?;
        if let WriteTarget::Project(root) = target {
            return project_target(key, &p, &root).map(|p| (self.paths.ultra.clone(), p));
        }
        let font = key == "terminal.font.size";
        if !font && !UI_KEYS.contains(&key) && !is_project_default(&p) {
            return Err("setting is not UI-writable".into());
        }
        let file = match target {
            WriteTarget::Machine => self.paths.machine_file.clone(),
            WriteTarget::Auto => self
                .sources
                .get(&p)
                .map(|s| PathBuf::from(&s.file))
                .unwrap_or_else(|| {
                    if font {
                        self.paths.terminal.clone()
                    } else {
                        self.paths.ultra.clone()
                    }
                }),
            WriteTarget::Project(_) => unreachable!(),
        };
        // terminal.toml spells the setting in Alacritty syntax.
        if font && file == self.paths.terminal {
            return Ok((file, vec!["font".into(), "size".into()]));
        }
        Ok((file, p))
    }
    pub fn set(
        &mut self,
        key: &str,
        value: serde_json::Value,
        target: WriteTarget,
    ) -> Result<(), String> {
        let (file, p) = self.target(key, target)?;
        let v = toml::Value::try_from(value).map_err(|e| e.to_string())?;
        let validation = if p == ["font", "size"] {
            font_size_key()
        } else {
            p.clone()
        };
        validate(&validation, &v)?;
        if validation.last().is_some_and(|s| s == "default_tool")
            && self.settings.tool(v.as_str().unwrap()).is_none()
        {
            return Err("unknown tool id".into());
        }
        write::edit(&file, &p, Some(&v))?;
        self.reload();
        Ok(())
    }
    pub fn reset(&mut self, key: &str, target: WriteTarget) -> Result<(), String> {
        let (file, p) = self.target(key, target)?;
        write::edit(&file, &p, None)?;
        self.reload();
        Ok(())
    }
    pub fn ensure_starter(&self, file: &Path) -> Result<(), String> {
        if ![
            &self.paths.ultra,
            &self.paths.terminal,
            &self.paths.keys,
            &self.paths.machine_file,
        ]
        .iter()
        .any(|p| p.as_path() == file)
        {
            return Err("not an app configuration file".into());
        }
        if file.exists() {
            return Ok(());
        }
        let text = if file == self.paths.terminal {
            "# Embedded terminal only; Alacritty syntax, merged over bundled defaults.\n# [font]\n# size = 15.0\n"
        } else if file == self.paths.keys {
            "# Native physical shortcuts. Empty string unbinds an action.\n[bindings]\n# palette = \"Ctrl+Shift+Space\" # Linux; use Cmd+O on macOS\n"
        } else {
            "# Ultra 9000 settings. Machine files override ultra.toml.\n# [launch]\n# default_tool = \"omp\"\n# [sidebar]\n# width = 304 # 240–360\n# density = \"comfortable\" # comfortable | compact\n# provider_mark = \"corner\" # corner | off\n# auto_collapse_below = 800\n# [icons]\n# brand_color = \"all\" # all | pickers | launch | off\n# [tray]\n# icon = \"light\" # light | dark\n# show = \"auto\" # auto | always | never; auto: hidden while a shell plugin shows Ultra 9000\n# [layout]\n# context_bar = \"auto\" # auto | always | never\n# [notify]\n# needs_you = true\n# turn_finished = false\n# [power]\n# keep_awake = \"off\" # off | working | running; prevents idle sleep while agents work or items run\n# [projects]\n# pinned = []\n# roots = [] # absolute paths; empty means all projects\n# [projects.\"/absolute/project\"]\n# default_tool = \"claude\"\n# [tools.custom]\n# command = \"custom\" # argv data, never shell-evaluated\n# args = []\n# resume = [\"--resume\", \"{id}\"]\n# glyph = \"ag\" # optional letters; omit for the generic agent mark\n# icon = \"icons/custom.svg\" # SVG or PNG, relative to this folder\n# icon_tint = \"mono\" # mono | original\n# icon_fill = false\n# accent = \"#7fdbca\" # colour of single-colour marks\n# [providers.example]\n# icon = \"icons/provider.svg\"\n"
        };
        write::atomic(file, text.as_bytes())
    }
}
fn strings(v: &toml::Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().into())
        .collect()
}
fn parse_key(key: &str) -> Result<Vec<String>, String> {
    let doc = format!("{key} = 0")
        .parse::<toml::Table>()
        .map_err(|e| e.to_string())?;
    let mut flat = Flat::new();
    flatten(&doc, &mut vec![], &mut flat);
    if flat.len() != 1 {
        return Err("expected one setting key".into());
    }
    Ok(flat.into_keys().next().unwrap())
}
/// A project target writes only `[projects."<root>"] default_tool` in ultra.toml.
fn project_target(key: &str, p: &[String], root: &Path) -> Result<Vec<String>, String> {
    let canonical = is_project_default(p);
    if canonical && Path::new(&p[1]) != root {
        return Err("project key does not match target".into());
    }
    if key != "launch.default_tool" && key != "default_tool" && !canonical {
        return Err("project target only supports default_tool".into());
    }
    if !root.is_absolute() {
        return Err("project path must be absolute".into());
    }
    Ok(vec![
        "projects".into(),
        root.to_string_lossy().into(),
        "default_tool".into(),
    ])
}
/// terminal.toml's `font.size` is the `terminal.font.size` setting.
fn setting_key(kind: FileKind, source_key: &[String]) -> Vec<String> {
    if kind == FileKind::Terminal && source_key == ["font", "size"] {
        font_size_key()
    } else {
        source_key.to_vec()
    }
}
fn is_validated(kind: FileKind, key: &[String]) -> bool {
    match kind {
        FileKind::Settings => true,
        FileKind::Terminal => key == FONT_SIZE,
        FileKind::Keys => false,
    }
}
/// Keeps a file's previous value (and its source) for a key that just became invalid.
fn retain_previous(
    previous: &FileState,
    key: Vec<String>,
    accepted: &mut Flat,
    sources: &mut BTreeMap<Vec<String>, Source>,
) {
    let Some(v) = previous.values.get(&key) else {
        return;
    };
    if let Some(source) = previous.sources.get(&key) {
        sources.insert(key.clone(), source.clone());
    }
    accepted.insert(key, v.clone());
}
fn is_tool_key(p: &[String]) -> bool {
    p.len() >= 3 && p[0] == "tools"
}
fn is_icon_key(p: &[String]) -> bool {
    p.len() == 3 && p[2] == "icon" && matches!(p[0].as_str(), "tools" | "providers")
}
fn is_project_default(p: &[String]) -> bool {
    p.len() == 3 && p[0] == "projects" && p[2] == "default_tool"
}
fn string(v: &toml::Value) -> String {
    v.as_str().unwrap().into()
}
/// The tool `id`, adding a generic agent for an id that is not built in.
fn tool_entry<'a>(s: &'a mut Settings, id: &str) -> &'a mut ToolSpec {
    let index = match s.tools.iter().position(|t| t.id == id) {
        Some(index) => index,
        None => {
            s.tools.push(ToolSpec {
                id: id.into(),
                label: id.into(),
                glyph: String::new(),
                icon: None,
                accent: None,
                kind: ToolKind::Agent,
                command: Some(id.into()),
                args: vec![],
                resume: None,
                sessions: None,
                status: StatusSource::Process,
                env: vec![],
            });
            s.tools.len() - 1
        }
    };
    &mut s.tools[index]
}
/// Applies `[tools.<id>] <field>` (or `env.<name>`) to a tool. Icons load separately.
fn apply_tool_field(t: &mut ToolSpec, field: &[String], v: &toml::Value) {
    match field[0].as_str() {
        "label" => t.label = string(v),
        "glyph" => t.glyph = string(v),
        "accent" => t.accent = Some(string(v).to_ascii_lowercase()),
        "command" => t.command = Some(string(v)),
        "kind" if v.as_str() == Some("terminal") => t.kind = ToolKind::Terminal,
        "kind" => t.kind = ToolKind::Agent,
        "args" => t.args = strings(v),
        "resume" => t.resume = Some(strings(v)),
        "sessions" => {
            t.sessions = Some(match v.as_str().unwrap() {
                "claude" => StoreKind::Claude,
                "codex" => StoreKind::Codex,
                "pi" => StoreKind::Pi,
                _ => StoreKind::Omp,
            })
        }
        "status" if v.as_str() == Some("omp") => t.status = StatusSource::Omp,
        "status" => t.status = StatusSource::Process,
        "env" => t.env.push((field[1].clone(), string(v))),
        _ => {}
    }
}
/// Applies one merged non-tool setting. The font size applies only when a file sets it.
fn apply_setting(s: &mut Settings, p: &[String], v: &toml::Value, sourced: bool) {
    match key_name(p).as_str() {
        "launch.default_tool" => s.default_tool = string(v),
        "sidebar.width" => s.sidebar_width = v.as_integer().unwrap() as u32,
        "sidebar.density" => s.sidebar_density = string(v),
        "sidebar.provider_mark" => s.sidebar_provider_mark = string(v),
        "icons.brand_color" => s.brand_color = string(v),
        "tray.icon" => s.tray_icon = string(v),
        "tray.show" => {
            s.tray_show = match v.as_str() {
                Some("always") => TrayShow::Always,
                Some("never") => TrayShow::Never,
                _ => TrayShow::Auto,
            }
        }
        "sidebar.auto_collapse_below" => {
            s.sidebar_auto_collapse_below = v.as_integer().unwrap() as u32
        }
        "layout.context_bar" => s.context_bar = string(v),
        "notify.needs_you" => s.notify_needs_you = v.as_bool().unwrap(),
        "notify.turn_finished" => s.notify_turn_finished = v.as_bool().unwrap(),
        "power.keep_awake" => {
            s.keep_awake = match v.as_str() {
                Some("working") => KeepAwake::Working,
                Some("running") => KeepAwake::Running,
                _ => KeepAwake::Off,
            }
        }
        "terminals.remove_on_clean_exit" => s.remove_on_clean_exit = v.as_bool().unwrap(),
        "terminals.confirm_close_with_foreground_job" => {
            s.confirm_close_with_foreground_job = v.as_bool().unwrap()
        }
        "confirm.close_unmonitored_agents" => {
            s.confirm_close_unmonitored_agents = v.as_bool().unwrap()
        }
        "resume.open_elsewhere_hint_seconds" => {
            s.open_elsewhere_hint_seconds = v.as_integer().unwrap() as u64
        }
        "file_links.markdown" => {
            s.markdown = if v.as_str() == Some("neovim") {
                MarkdownRoute::Neovim
            } else {
                MarkdownRoute::View
            }
        }
        "projects.pinned" => s.pinned = strings(v).into_iter().map(PathBuf::from).collect(),
        "projects.roots" => s.roots = strings(v).into_iter().map(PathBuf::from).collect(),
        "terminal.font.size" if sourced => {
            s.terminal_font_size = Some(
                v.as_float()
                    .unwrap_or_else(|| v.as_integer().unwrap() as f64) as f32,
            )
        }
        _ if p.len() == 3 && p[0] == "projects" => {
            let entry = s.projects.entry(PathBuf::from(&p[1])).or_default();
            if p[2] == "name" {
                entry.name = Some(string(v));
            } else {
                entry.default_tool = Some(string(v));
            }
        }
        _ => {}
    }
}
/// Applies a loaded user image to its provider, or to its tool with tint and fill.
fn apply_icon(s: &mut Settings, values: &Flat, p: &[String], url: &str) {
    if p[0] == "providers" {
        s.provider_icons.insert(p[1].clone(), url.to_owned());
        return;
    }
    let Some(tool) = s.tools.iter_mut().find(|t| t.id == p[1]) else {
        return;
    };
    let setting = |name: &str| values.get(&vec!["tools".into(), p[1].clone(), name.into()]);
    tool.icon = Some(ToolIcon {
        url: url.to_owned(),
        tint: if setting("icon_tint").and_then(toml::Value::as_str) == Some("original") {
            IconTint::Original
        } else {
            IconTint::Mono
        },
        fill: setting("icon_fill")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false),
    });
}
/// The last good launch default if its tool still exists, else OMP; for a project,
/// its last good default if that tool still exists, else the launch default.
fn fallback_tool(s: &Settings, before: &Settings, p: &[String]) -> String {
    if p[0] != "launch" {
        return before
            .projects
            .get(Path::new(&p[1]))
            .and_then(|p| p.default_tool.clone())
            .filter(|id| s.tool(id).is_some())
            .unwrap_or_else(|| s.default_tool.clone());
    }
    if s.tool(&before.default_tool).is_some() {
        before.default_tool.clone()
    } else {
        "omp".into()
    }
}
#[cfg(test)]
mod tests;
