//! Comment-preserving TOML edits and atomic sibling-file replacement.
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
use toml_edit::{DocumentMut, Item, RawString, Table, TableLike};
static NEXT: AtomicU64 = AtomicU64::new(0);
/// File-backed app settings are deliberately small; reject before allocating a full document.
pub(crate) const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
pub(crate) fn read_bounded(path: &Path) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "configuration exceeds 1 MiB; retaining last good file",
        ));
    }
    String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
pub(crate) fn atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("file has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = parent.join(format!(
        ".u9k-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        fs::File::open(parent)?.sync_all()
    })()
    .map_err(|e: std::io::Error| e.to_string());
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
pub(crate) fn edit(path: &Path, key: &[String], value: Option<&toml::Value>) -> Result<(), String> {
    let text = match read_bounded(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    let mut doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    let mut comments = String::new();
    edit_table(doc.as_item_mut(), key, value, &mut comments)?;
    if !comments.is_empty() {
        let existing = doc
            .as_table()
            .decor()
            .suffix()
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let suffix = format!("{existing}\n{comments}");
        doc.as_table_mut().decor_mut().set_suffix(suffix);
    }
    atomic(path, doc.to_string().as_bytes())
}

fn edit_table(
    parent: &mut Item,
    key: &[String],
    value: Option<&toml::Value>,
    comments: &mut String,
) -> Result<(), String> {
    let (leaf, rest) = key.split_first().ok_or("empty key")?;
    let inline = parent.is_inline_table();
    let table = parent
        .as_table_like_mut()
        .ok_or("key parent is not a table")?;
    if !rest.is_empty() {
        if !table.contains_key(leaf) {
            if value.is_none() {
                return Ok(());
            }
            table.insert(leaf, empty_table(inline));
        }
        return edit_table(table.get_mut(leaf).unwrap(), rest, value, comments);
    }
    match value {
        Some(value) => set_value(table, leaf, value),
        None => {
            remove_value(table, leaf, comments);
            Ok(())
        }
    }
}

fn empty_table(inline: bool) -> Item {
    if inline {
        Item::Value(toml_edit::Value::InlineTable(toml_edit::InlineTable::new()))
    } else {
        Item::Table(Table::new())
    }
}

fn set_value(table: &mut dyn TableLike, leaf: &str, value: &toml::Value) -> Result<(), String> {
    let parsed = format!("value = {value}")
        .parse::<DocumentMut>()
        .map_err(|e| e.to_string())?;
    let mut new = parsed["value"].clone();
    if let (Some(old), Some(new)) = (table.get(leaf).and_then(Item::as_value), new.as_value_mut()) {
        *new.decor_mut() = old.decor().clone();
    }
    if let Some(old) = table.get_mut(leaf) {
        *old = new;
    } else {
        table.insert(leaf, new);
    }
    Ok(())
}

/// Comments attached to a removed value are user-owned too: leave them in the gap.
fn remove_value(table: &mut dyn TableLike, leaf: &str, comments: &mut String) {
    if let Some(key) = table.key(leaf) {
        keep_comments(
            [key.leaf_decor().prefix(), key.leaf_decor().suffix()],
            comments,
        );
    }
    if let Some(old) = table.get(leaf).and_then(Item::as_value) {
        keep_comments([old.decor().prefix(), old.decor().suffix()], comments);
    }
    table.remove(leaf);
}

fn keep_comments(decor: [Option<&RawString>; 2], comments: &mut String) {
    for s in decor {
        if let Some(s) = s.and_then(RawString::as_str).filter(|s| s.contains('#')) {
            comments.push_str(s);
            comments.push('\n');
        }
    }
}
