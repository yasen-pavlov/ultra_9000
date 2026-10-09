//! load(path) and save(path, state): UI memory only, never agent conversation content.
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiStateFile {
    pub sidebar_expanded: bool,
    pub folded_projects: Vec<String>,
    pub last_set: Vec<LastSetEntry>,
}
impl Default for UiStateFile {
    fn default() -> Self {
        Self {
            sidebar_expanded: true,
            folded_projects: vec![],
            last_set: vec![],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastSetEntry {
    pub tool: String,
    pub title: String,
    pub project_id: String,
    pub cwd: String,
    pub conversation_id: String,
    pub conversation_path: String,
}
pub fn load(path: &Path) -> UiStateFile {
    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(_) => {
                log::warn!("corrupt UI state {}; using defaults", path.display());
                UiStateFile::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => UiStateFile::default(),
        Err(e) => {
            log::warn!("cannot read UI state {}: {e}", path.display());
            UiStateFile::default()
        }
    }
}
pub fn save(path: &Path, state: &UiStateFile) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
    crate::config::atomic_write(path, &bytes)
}
#[cfg(test)]
mod tests;
