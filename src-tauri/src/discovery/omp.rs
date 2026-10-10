//! OMP fixed title slot and legacy headers; authoritative slot, bounded reads.
use super::{conversation, metadata::Record, model, records, timestamp, windows, Conversation};
use std::{io, path::Path};
pub(super) fn read(tool: &str, path: &Path, updated: i64) -> io::Result<Option<Conversation>> {
    let w = windows(path, 4 * 1024, 32 * 1024)?;
    parse(tool, path, &w.prefix, &w.tail, updated)
}
pub(super) fn read_bound(
    tool: &str,
    path: &Path,
    updated: i64,
) -> io::Result<Option<Conversation>> {
    let w = windows(path, 4 * 1024, 32 * 1024)?;
    parse_with_stubs(tool, path, &w.prefix, &w.tail, updated, false)
}
fn parse(
    tool: &str,
    path: &Path,
    prefix: &[u8],
    tail: &[u8],
    updated: i64,
) -> io::Result<Option<Conversation>> {
    parse_with_stubs(tool, path, prefix, tail, updated, true)
}
fn parse_with_stubs(
    tool: &str,
    path: &Path,
    prefix: &[u8],
    tail: &[u8],
    updated: i64,
    hide_stubs: bool,
) -> io::Result<Option<Conversation>> {
    let first = prefix.split(|b| *b == b'\n').next().unwrap_or_default();
    let slot = serde_json::from_slice::<Record>(first)
        .ok()
        .filter(|r| {
            r.kind == "title"
                && r.v == Some(1)
                && r.title.is_some()
                && r.updated_at.is_some()
                && r.pad.is_some()
                && first.len() + 1 == 256
        })
        .and_then(|r| r.title);
    let mut header = None;
    let mut latest = None;
    let mut assistant = false;
    for r in records(prefix).chain(records(tail)) {
        if r.kind == "session" && header.is_none() {
            header = Some(r);
            continue;
        }
        assistant |= r.kind == "message"
            && r.message.as_ref().and_then(|m| m.role.as_deref()) == Some("assistant");
        if let Some(m) = model(&r) {
            latest = Some(m);
        }
    }
    let Some(h) = header else {
        return Ok(None);
    };
    let title = slot.or(h.title).filter(|t| !t.trim().is_empty());
    if hide_stubs && title.is_none() && !assistant {
        return Ok(None);
    }
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
