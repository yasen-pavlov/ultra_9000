//! Settings schema, built-in tools and wire DTOs. ConfigStore reexports these types.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Agent,
    Terminal,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreKind {
    Omp,
    Claude,
    Codex,
    Pi,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusSource {
    Omp,
    Process,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconTint {
    Mono,
    Original,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolIcon {
    pub url: String,
    pub tint: IconTint,
    pub fill: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub id: String,
    pub label: String,
    pub glyph: String,
    pub icon: Option<ToolIcon>,
    /// `#rrggbb` mark colour for single-colour marks where colour shows (board 24).
    pub accent: Option<String>,
    pub kind: ToolKind,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub resume: Option<Vec<String>>,
    pub sessions: Option<StoreKind>,
    pub status: StatusSource,
    pub env: Vec<(String, String)>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectOverride {
    pub name: Option<String>,
    pub default_tool: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub default_tool: String,
    pub sidebar_width: u32,
    pub sidebar_density: String,
    pub sidebar_provider_mark: String,
    pub brand_color: String,
    pub tray_icon: String,
    pub provider_icons: BTreeMap<String, String>,
    pub sidebar_auto_collapse_below: u32,
    pub context_bar: String,
    pub notify_needs_you: bool,
    pub notify_turn_finished: bool,
    pub remove_on_clean_exit: bool,
    pub confirm_close_with_foreground_job: bool,
    pub confirm_close_unmonitored_agents: bool,
    pub open_elsewhere_hint_seconds: u64,
    pub pinned: Vec<PathBuf>,
    pub roots: Vec<PathBuf>,
    pub projects: BTreeMap<PathBuf, ProjectOverride>,
    pub tools: Vec<ToolSpec>,
    pub terminal_font_size: Option<f32>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            default_tool: "omp".into(),
            sidebar_width: 304,
            sidebar_density: "comfortable".into(),
            sidebar_provider_mark: "corner".into(),
            brand_color: "all".into(),
            tray_icon: "light".into(),
            provider_icons: BTreeMap::new(),
            sidebar_auto_collapse_below: 800,
            context_bar: "auto".into(),
            notify_needs_you: true,
            notify_turn_finished: false,
            remove_on_clean_exit: true,
            confirm_close_with_foreground_job: true,
            confirm_close_unmonitored_agents: false,
            open_elsewhere_hint_seconds: 120,
            pinned: vec![],
            roots: vec![],
            projects: BTreeMap::new(),
            tools: builtins(),
            terminal_font_size: None,
        }
    }
}
impl Settings {
    pub fn tool(&self, id: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|t| t.id == id)
    }
    pub fn project_default_tool(&self, root: &Path) -> &str {
        self.projects
            .get(root)
            .and_then(|p| p.default_tool.as_deref())
            .unwrap_or(&self.default_tool)
    }
    pub fn project_name(&self, root: &Path) -> String {
        self.projects
            .get(root)
            .and_then(|p| p.name.clone())
            .unwrap_or_else(|| {
                root.file_name()
                    .unwrap_or(root.as_os_str())
                    .to_string_lossy()
                    .into()
            })
    }
}
pub fn builtins() -> Vec<ToolSpec> {
    [
        (
            "omp",
            "OMP",
            "π",
            Some("omp"),
            Some(StoreKind::Omp),
            Some(vec!["--resume", "{path}"]),
        ),
        (
            "claude",
            "Claude Code",
            "CC",
            Some("claude"),
            Some(StoreKind::Claude),
            Some(vec!["--resume", "{id}"]),
        ),
        (
            "codex",
            "Codex",
            "cx",
            Some("codex"),
            Some(StoreKind::Codex),
            Some(vec!["resume", "{id}"]),
        ),
        (
            "pi",
            "pi",
            "pi",
            Some("pi"),
            Some(StoreKind::Pi),
            Some(vec!["--session", "{path}"]),
        ),
        ("shell", "Shell", ">_", None, None, None),
        ("neovim", "Neovim", "nv", Some("nvim"), None, None),
    ]
    .into_iter()
    .map(|(id, label, glyph, command, sessions, resume)| ToolSpec {
        id: id.into(),
        label: label.into(),
        glyph: glyph.into(),
        icon: None,
        accent: match id {
            "omp" => Some("#7fdbca".into()),
            "shell" => Some("#c3e88d".into()),
            _ => None,
        },
        kind: if sessions.is_some() {
            ToolKind::Agent
        } else {
            ToolKind::Terminal
        },
        command: command.map(Into::into),
        args: vec![],
        resume: resume.map(|a| a.into_iter().map(Into::into).collect()),
        sessions,
        status: if id == "omp" {
            StatusSource::Omp
        } else {
            StatusSource::Process
        },
        env: if id == "omp" {
            vec![("WARP_CLI_AGENT_PROTOCOL_VERSION".into(), "1".into())]
        } else {
            vec![]
        },
    })
    .collect()
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub file: String,
    pub line: usize,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingValue {
    pub key: String,
    pub value: serde_json::Value,
    pub source: Option<Source>,
    pub default_value: serde_json::Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsError {
    pub file: String,
    pub line: Option<usize>,
    pub key: Option<String>,
    pub message: String,
    pub last_good: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsFile {
    pub name: String,
    pub path: String,
    pub exists: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsState {
    pub revision: u64,
    pub config_dir: String,
    pub machine: String,
    pub files: Vec<SettingsFile>,
    pub values: Vec<SettingValue>,
    pub errors: Vec<SettingsError>,
    pub reloaded_at: u64,
}
pub const UI_KEYS: &[&str] = &[
    "launch.default_tool",
    "sidebar.width",
    "sidebar.density",
    "sidebar.provider_mark",
    "icons.brand_color",
    "tray.icon",
    "sidebar.auto_collapse_below",
    "layout.context_bar",
    "notify.needs_you",
    "notify.turn_finished",
    "terminals.remove_on_clean_exit",
    "terminals.confirm_close_with_foreground_job",
    "confirm.close_unmonitored_agents",
    "resume.open_elsewhere_hint_seconds",
    "projects.pinned",
    "terminal.font.size",
];
#[derive(Clone, Debug)]
pub enum WriteTarget {
    Auto,
    Machine,
    Project(PathBuf),
}
impl Serialize for WriteTarget {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => s.serialize_str("auto"),
            Self::Machine => s.serialize_str("machine"),
            Self::Project(project) => serde_json::json!({"project":project}).serialize(s),
        }
    }
}
impl<'de> Deserialize<'de> for WriteTarget {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        match v {
            serde_json::Value::String(s) if s == "auto" => Ok(Self::Auto),
            serde_json::Value::String(s) if s == "machine" => Ok(Self::Machine),
            serde_json::Value::Object(o) if o.len() == 1 => o
                .get("project")
                .and_then(|v| v.as_str())
                .map(|p| Self::Project(PathBuf::from(p)))
                .ok_or_else(|| serde::de::Error::custom("expected project path")),
            _ => Err(serde::de::Error::custom(
                "expected auto, machine or {project}",
            )),
        }
    }
}
pub(crate) fn defaults() -> BTreeMap<Vec<String>, toml::Value> {
    let mut map = BTreeMap::new();
    for (k, v) in [
        ("launch.default_tool", toml::Value::String("omp".into())),
        ("sidebar.width", 304.into()),
        ("sidebar.density", "comfortable".into()),
        ("sidebar.provider_mark", "corner".into()),
        ("icons.brand_color", "all".into()),
        ("tray.icon", "light".into()),
        ("sidebar.auto_collapse_below", 800.into()),
        ("layout.context_bar", "auto".into()),
        ("notify.needs_you", true.into()),
        ("notify.turn_finished", false.into()),
        ("terminals.remove_on_clean_exit", true.into()),
        ("terminals.confirm_close_with_foreground_job", true.into()),
        ("confirm.close_unmonitored_agents", false.into()),
        ("resume.open_elsewhere_hint_seconds", 120.into()),
        ("projects.pinned", toml::Value::Array(vec![])),
        ("projects.roots", toml::Value::Array(vec![])),
        ("terminal.font.size", toml::Value::Float(15.0)),
    ] {
        map.insert(k.split('.').map(Into::into).collect(), v);
    }
    map
}
pub(crate) fn key_name(path: &[String]) -> String {
    path.iter()
        .map(|s| {
            if s.contains(['.', '/', '"', '\\']) {
                serde_json::to_string(s).unwrap()
            } else {
                s.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}
pub(crate) fn validate(path: &[String], v: &toml::Value) -> Result<(), String> {
    let key = key_name(path);
    let string = || v.as_str().is_some_and(|s| !s.trim().is_empty());
    let strings = || {
        v.as_array()
            .is_some_and(|a| a.iter().all(|x| x.as_str().is_some()))
    };
    let enum_is = |values: &[&str]| v.as_str().is_some_and(|s| values.contains(&s));
    let range = |a, b| v.as_integer().is_some_and(|n| n >= a && n <= b);
    let (ok, expected) = match key.as_str() {
        "launch.default_tool" => (string(), "nonempty tool id"),
        "sidebar.width" => (range(240, 360), "integer 240–360"),
        "sidebar.density" => (
            enum_is(&["comfortable", "compact"]),
            "comfortable | compact",
        ),
        "sidebar.provider_mark" => (enum_is(&["corner", "off"]), "corner | off"),
        "icons.brand_color" => (
            enum_is(&["all", "pickers", "launch", "off"]),
            "all | pickers | launch | off",
        ),
        "tray.icon" => (enum_is(&["light", "dark"]), "light | dark"),
        "sidebar.auto_collapse_below" => (range(0, 10000), "integer 0–10000"),
        "layout.context_bar" => (
            enum_is(&["auto", "always", "never"]),
            "auto | always | never",
        ),
        "notify.needs_you"
        | "notify.turn_finished"
        | "terminals.remove_on_clean_exit"
        | "terminals.confirm_close_with_foreground_job"
        | "confirm.close_unmonitored_agents" => (v.is_bool(), "boolean"),
        "resume.open_elsewhere_hint_seconds" => (range(0, 86400), "integer 0–86400"),
        "projects.pinned" | "projects.roots" => (
            strings()
                && v.as_array()
                    .unwrap()
                    .iter()
                    .all(|p| Path::new(p.as_str().unwrap()).is_absolute()),
            "array of absolute paths",
        ),
        "terminal.font.size" => (
            v.as_float()
                .or_else(|| v.as_integer().map(|n| n as f64))
                .is_some_and(|n| n.is_finite() && (4.0..=96.0).contains(&n)),
            "number 4–96",
        ),
        _ if path.len() == 3
            && path[0] == "projects"
            && Path::new(&path[1]).is_absolute()
            && matches!(path[2].as_str(), "name" | "default_tool") =>
        {
            (string(), "nonempty string")
        }
        _ if path.len() == 3 && path[0] == "providers" && path[2] == "icon" => {
            (string(), "nonempty relative icon path")
        }
        _ if path.len() >= 3 && path[0] == "tools" => match path[2].as_str() {
            "label" | "glyph" | "command" if path.len() == 3 => (string(), "nonempty string"),
            "icon" if path.len() == 3 => (string(), "nonempty relative icon path"),
            "icon_tint" if path.len() == 3 => (enum_is(&["mono", "original"]), "mono | original"),
            "icon_fill" if path.len() == 3 => (v.is_bool(), "boolean"),
            "accent" if path.len() == 3 => (
                v.as_str().is_some_and(|s| {
                    s.len() == 7
                        && s.starts_with('#')
                        && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
                }),
                "#rrggbb colour",
            ),
            "args" | "resume" if path.len() == 3 => (strings(), "string array"),
            "kind" if path.len() == 3 => (enum_is(&["agent", "terminal"]), "agent | terminal"),
            "sessions" if path.len() == 3 => (
                enum_is(&["omp", "claude", "codex", "pi"]),
                "omp | claude | codex | pi",
            ),
            "status" if path.len() == 3 => (enum_is(&["omp", "process"]), "omp | process"),
            "env" if path.len() == 4 => (
                v.is_str() && !path[3].contains(['=', '\0']) && !v.as_str().unwrap().contains('\0'),
                "environment string",
            ),
            _ => return Err("unknown key".into()),
        },
        _ => return Err("unknown key".into()),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("expected {expected}"))
    }
}
#[cfg(test)]
mod tests;
