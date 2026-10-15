//! Desktop notifications, sent as Ultra 9000 itself. Notification text is data, never
//! executable source. Clicking one shows the window and selects the item that raised it; a
//! newer notification for the same item replaces the older one.
use crate::{lifecycle, workspace};
use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
    sync::LazyLock,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Runtime};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "macos")]
pub use macos::install;

/// Announce `summary`/`body` for `item`. Fire-and-forget: failures are only logged.
pub fn desktop(item: &str, summary: &str, body: &str) {
    platform::send(item, summary, body);
}

/// Random per-run identity: Notification Center can persist a notification across
/// reboots, so a pid (which can be reused) is not enough to identify its originating run.
static RUN_TOKEN: LazyLock<String> = LazyLock::new(|| {
    let mut token = RandomState::new().build_hasher();
    token.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    format!("{:016x}", token.finish())
});

fn identifier(item: &str) -> String {
    format!("{}/{item}", *RUN_TOKEN)
}

/// The item a clicked notification belongs to, if this process sent it.
fn item_of(identifier: &str) -> Option<&str> {
    let (run, item) = identifier.split_once('/')?;
    let suffix = item.strip_prefix("item-")?;
    (run == RUN_TOKEN.as_str()
        && !suffix.is_empty()
        && suffix.bytes().all(|c| c.is_ascii_digit())
        && !suffix.starts_with('0'))
    .then_some(item)
}

/// A notification was clicked (any thread): show the window and select its item.
/// `activation_token` is the Wayland activation token the notification server sent, if any.
fn clicked(identifier: &str, activation_token: Option<String>) {
    let item = item_of(identifier).map(str::to_owned);
    crate::platform::defer_on_main(Box::new(move || {
        let result = workspace::app_handle()
            .and_then(|app| open_item(&app, item.as_deref(), activation_token.as_deref()));
        if let Err(e) = result {
            log::warn!("notification click: {e}");
        }
    }));
}

/// Main thread. The item may have closed since: then only the window is shown.
fn open_item<R: Runtime>(
    app: &AppHandle<R>,
    item: Option<&str>,
    activation_token: Option<&str>,
) -> Result<(), String> {
    if workspace::is_quitting()? {
        return Ok(());
    }
    lifecycle::show_window_with_activation(app, activation_token, None)?;
    match item {
        Some(item) if workspace::has_item(item)? => workspace::select(item),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
