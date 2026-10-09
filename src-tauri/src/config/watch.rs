//! watch(dir, callback) returns a Send guard. Dropping it stops the watcher and worker.
use ::notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::{path::Path, sync::mpsc, thread, time::Duration};
struct Guard {
    watcher: Option<RecommendedWatcher>,
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.watcher.take();
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
pub fn watch(dir: &Path, on_change: impl Fn() + Send + 'static) -> Result<Box<dyn Send>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::sync_channel(1);
    let (stop, stopped) = mpsc::channel();
    let mut watcher =
        ::notify::recommended_watcher(move |event: Result<::notify::Event, ::notify::Error>| {
            match event {
                Ok(e) if !matches!(e.kind, ::notify::EventKind::Access(_)) => {
                    let _ = tx.try_send(());
                }
                Err(e) => log::warn!("config watcher: {e}"),
                _ => {}
            }
        })
        .map_err(|e| e.to_string())?;
    watcher
        .watch(dir, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;
    let worker = thread::Builder::new()
        .name("u9k-config-watch".into())
        .spawn(move || debounce(&rx, &stopped, QUIET, on_change))
        .map_err(|e| e.to_string())?;
    Ok(Box::new(Guard {
        watcher: Some(watcher),
        stop,
        worker: Some(worker),
    }))
}
/// Reloads wait until changes have been quiet this long.
const QUIET: Duration = Duration::from_millis(150);
/// Calls `on_change` once after wakeups have been quiet for `quiet`. Returns on a stop
/// request or when the watcher side disconnects, without flushing a pending change.
fn debounce(
    rx: &mpsc::Receiver<()>,
    stopped: &mpsc::Receiver<()>,
    quiet: Duration,
    on_change: impl Fn(),
) {
    let mut pending = false;
    loop {
        if stopped.try_recv().is_ok() {
            break;
        }
        match rx.recv_timeout(quiet) {
            Ok(()) => pending = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if pending {
                    pending = false;
                    on_change();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests;
