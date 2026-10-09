//! File-backed settings: ConfigPaths, Settings/ToolSpec, ConfigStore and watch.
//! Load/reload/write calls block; run them on a worker, never with a workspace borrow.
//! terminal_toml is the last-good raw file. Merge over bundled terminal defaults,
//! then overlay settings().terminal_font_size, which supports machine scope.
mod icons;
mod paths;
mod schema;
mod store;
mod watch;
mod write;
pub type ConfigPaths = paths::ConfigPaths;
pub use schema::*;
pub type ConfigStore = store::ConfigStore;
pub type Reloaded = store::Reloaded;
pub fn watch(
    dir: &std::path::Path,
    on_change: impl Fn() + Send + 'static,
) -> Result<Box<dyn Send>, String> {
    watch::watch(dir, on_change)
}
pub(crate) fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    write::atomic(path, bytes)
}
#[cfg(test)]
mod tests;
