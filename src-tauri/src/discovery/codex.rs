//! Codex rollout metadata plus its title-only index; no SQLite or prompt fields.
use super::{conversation, metadata::ThreadSource, records, timestamp, windows, Conversation};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, BufRead, Read},
    path::Path,
};
#[derive(Deserialize)]
struct Title {
    id: String,
    thread_name: String,
}
const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TITLE_BYTES: usize = 8 * 1024 * 1024;
const MAX_TITLE_ENTRIES: usize = 50_000;
fn index_limit() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Codex title index exceeds metadata resource limit",
    )
}
pub(super) fn titles(path: &Path) -> io::Result<BTreeMap<String, String>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e),
    };
    if file.metadata()?.len() > MAX_INDEX_BYTES {
        return Err(index_limit());
    }
    let mut reader = io::BufReader::new(file.take(MAX_INDEX_BYTES + 1));
    let mut map = BTreeMap::<String, String>::new();
    let mut scanned = 0u64;
    let mut retained = 0usize;
    let mut line = Vec::with_capacity(4096);
    loop {
        line.clear();
        let n = reader
            .by_ref()
            .take(64 * 1024)
            .read_until(b'\n', &mut line)?;
        scanned += n as u64;
        if scanned > MAX_INDEX_BYTES {
            return Err(index_limit());
        }
        if n == 0 {
            break;
        }
        if !line.ends_with(b"\n") && n == 64 * 1024 {
            skip_rest_of_line(&mut reader, &mut scanned)?;
            continue;
        }
        if let Ok(t) = serde_json::from_slice::<Title>(&line) {
            retain_title(&mut map, &mut retained, t)?;
        }
    }
    Ok(map)
}
/// Discards the remainder of an overlong index line, still counting it against the scan limit.
fn skip_rest_of_line(reader: &mut impl BufRead, scanned: &mut u64) -> io::Result<()> {
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok(());
        }
        let pos = buf.iter().position(|b| *b == b'\n');
        let n = pos.map(|n| n + 1).unwrap_or(buf.len());
        reader.consume(n);
        *scanned += n as u64;
        if *scanned > MAX_INDEX_BYTES {
            return Err(index_limit());
        }
        if pos.is_some() {
            return Ok(());
        }
    }
}
/// Keeps the latest non-blank title per id within the retained byte and entry limits.
fn retain_title(
    map: &mut BTreeMap<String, String>,
    retained: &mut usize,
    t: Title,
) -> io::Result<()> {
    if t.thread_name.trim().is_empty() {
        return Ok(());
    }
    *retained = retained.saturating_sub(map.get(&t.id).map_or(0, |old| t.id.len() + old.len()));
    *retained += t.id.len() + t.thread_name.len();
    if *retained > MAX_TITLE_BYTES || !map.contains_key(&t.id) && map.len() >= MAX_TITLE_ENTRIES {
        return Err(index_limit());
    }
    map.insert(t.id, t.thread_name);
    Ok(())
}
pub(super) fn read(tool: &str, path: &Path, updated: i64) -> io::Result<Option<Conversation>> {
    let w = windows(path, 64 * 1024, 32 * 1024)?;
    let mut header = None;
    let mut latest = None;
    for r in records(&w.prefix).chain(records(&w.tail)) {
        if r.kind == "session_meta" && header.is_none() {
            header = r.payload;
        } else if r.kind == "turn_context" {
            if let Some(m) = r.payload.and_then(|p| p.model) {
                latest = Some(m);
            }
        }
    }
    let Some(h) = header else {
        return Ok(None);
    };
    if h.thread_source.as_deref().is_some_and(|s| s != "user")
        || matches!(h.source, Some(ThreadSource::Other(_)))
        || matches!(&h.source,Some(ThreadSource::Name(s)) if !matches!(s.as_str(),"cli"|"user"|"codex-tui"))
    {
        return Ok(None);
    }
    let _provider = h.model_provider;
    Ok(match (h.id.or(h.session_id), h.cwd) {
        (Some(id), Some(cwd)) => conversation(
            tool,
            path,
            id,
            cwd,
            None,
            timestamp(h.timestamp.as_deref()),
            updated,
            latest,
        ),
        _ => None,
    })
}
#[cfg(test)]
mod tests;
