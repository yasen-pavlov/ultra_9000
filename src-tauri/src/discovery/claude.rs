//! Claude root session files only, explicit titles only; prompt-derived fields are skipped.
use super::{conversation, records, timestamp, windows, Conversation};
use std::{io, path::Path};
pub(super) fn read(tool: &str, path: &Path, updated: i64) -> io::Result<Option<Conversation>> {
    let w = windows(path, 64 * 1024, 32 * 1024)?;
    let mut id = None;
    let mut cwd = None;
    let mut created = 0;
    let mut custom = None;
    let mut ai = None;
    let mut assistant = false;
    let mut latest = None;
    for r in records(&w.prefix).chain(records(&w.tail)) {
        if r.session_kind.as_deref() == Some("bg") || r.is_sidechain {
            return Ok(None);
        }
        if id.is_none() {
            id = r.session_id.clone();
        }
        if cwd.is_none() {
            cwd = r.cwd.clone();
        }
        if created == 0 {
            created = timestamp(r.timestamp.as_deref());
        }
        if r.kind == "custom-title" {
            custom = r.custom_title;
        } else if r.kind == "ai-title" {
            ai = r.ai_title;
        }
        if r.kind == "assistant" {
            assistant = true;
            if let Some(m) = r.message.and_then(|m| m.model) {
                latest = Some(m);
            }
        }
    }
    let title = custom.or(ai).filter(|t| !t.trim().is_empty());
    if !assistant && title.is_none() {
        return Ok(None);
    }
    Ok(match (id, cwd) {
        (Some(id), Some(cwd)) => conversation(tool, path, id, cwd, title, created, updated, latest),
        _ => None,
    })
}
#[cfg(test)]
mod tests;
