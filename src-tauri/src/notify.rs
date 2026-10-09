//! Desktop notifications are fire-and-forget. Notification text is never executable source.
pub fn desktop(summary: &str, body: &str) {
    let summary = summary.to_owned();
    let body = body.to_owned();
    if let Err(e) = std::thread::Builder::new().name("u9k-notify".into()).spawn(move || {
        #[cfg(target_os = "linux")]
        if let Err(e) = notify_rust::Notification::new().appname("Ultra 9000").summary(&summary).body(&body).show() {
            log::warn!("desktop notification failed: {e}");
        }
        #[cfg(target_os = "macos")]
        {
            // AppleScript's argv is data, including quotes/newlines. No Apple SDK build dependency.
            const SCRIPT: &str = "on run argv\n display notification (item 1 of argv) with title (item 2 of argv)\nend run";
            match std::process::Command::new("/usr/bin/osascript").args(["-e", SCRIPT, "--", &body, &summary])
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status() {
                Ok(status) if status.success() => {},
                Ok(status) => log::warn!("desktop notification exited with {status}"),
                Err(e) => log::warn!("desktop notification failed: {e}"),
            }
        }
    }) { log::warn!("notification worker failed: {e}"); }
}
