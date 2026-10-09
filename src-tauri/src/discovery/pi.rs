//! pi session metadata. Never use the first user message as a title.
use super::{conversation, model, records, timestamp, windows, Conversation};
use std::{io, path::Path};
pub(super) fn read(tool: &str, path: &Path, updated: i64) -> io::Result<Option<Conversation>> {
    let w = windows(path, 4 * 1024, 32 * 1024)?;
    let mut header = None;
    let mut title = None;
    let mut latest = None;
    for r in records(&w.prefix).chain(records(&w.tail)) {
        if r.kind == "session" && header.is_none() {
            header = Some(r);
            continue;
        }
        if r.kind == "session_info" {
            title = r.name.clone();
        }
        if let Some(m) = model(&r) {
            latest = Some(m);
        }
    }
    let Some(h) = header else {
        return Ok(None);
    };
    Ok(match (h.id, h.cwd) {
        (Some(id), Some(cwd)) => conversation(
            tool,
            path,
            id,
            cwd,
            title,
            timestamp(h.timestamp.as_deref()),
            updated,
            latest,
        ),
        _ => None,
    })
}
#[cfg(test)]
mod tests;
