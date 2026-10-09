use super::*;
use std::{fs, path::PathBuf, sync::Arc, time::Instant};
/// Upper bound for anything that should happen promptly.
const WAIT: Duration = Duration::from_secs(10);

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("u9k-watch-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// Runs `debounce` on a thread; the returned channel reports each change, then
/// disconnects once the worker has returned.
fn spawn_debounce(
    rx: mpsc::Receiver<()>,
    stopped: mpsc::Receiver<()>,
    quiet: Duration,
) -> mpsc::Receiver<()> {
    let (changes, changed) = mpsc::channel();
    thread::spawn(move || {
        debounce(&rx, &stopped, quiet, || changes.send(()).unwrap());
    });
    changed
}

#[test]
fn queued_wakeups_coalesce_into_one_change_per_quiet_period() {
    let (tx, rx) = mpsc::channel();
    let (_stop, stopped) = mpsc::channel();
    for _ in 0..5 {
        tx.send(()).unwrap();
    }
    let changed = spawn_debounce(rx, stopped, Duration::from_millis(20));
    changed.recv_timeout(WAIT).unwrap();
    tx.send(()).unwrap();
    changed.recv_timeout(WAIT).unwrap();
    drop(tx);
    // The worker returns on disconnect; no third change was ever produced.
    assert_eq!(
        changed.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}

#[test]
fn disconnect_drops_a_pending_change_and_stop_ends_the_worker() {
    let (tx, rx) = mpsc::channel();
    let (_stop, stopped) = mpsc::channel();
    tx.send(()).unwrap();
    drop(tx);
    let changed = spawn_debounce(rx, stopped, Duration::from_millis(20));
    assert_eq!(
        changed.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
    // A stop request ends the worker even while the watcher side stays connected.
    let (_tx, rx) = mpsc::channel();
    let (stop, stopped) = mpsc::channel();
    stop.send(()).unwrap();
    let changed = spawn_debounce(rx, stopped, Duration::from_millis(20));
    assert_eq!(
        changed.recv_timeout(WAIT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}

#[test]
fn watching_a_file_path_fails() {
    let root = scratch("file");
    fs::create_dir_all(&root).unwrap();
    let file = root.join("ultra.toml");
    fs::write(&file, "").unwrap();
    assert!(watch(&file.join("dir"), || {}).err().is_some());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_changes_call_back_and_dropping_the_guard_joins_the_worker() {
    let root = scratch("live");
    let dir = root.join("nested/config");
    let (changes, changed) = mpsc::channel();
    let token = Arc::new(());
    let held = token.clone();
    // Through the public entry point the app uses.
    let guard = crate::config::watch(&dir, move || {
        let _ = &held;
        let _ = changes.send(());
    })
    .unwrap();
    assert!(dir.is_dir(), "watch creates the directory");
    // Some backends deliver their first event late; keep editing until one arrives.
    let deadline = Instant::now() + WAIT;
    let mut n = 0;
    while changed.recv_timeout(Duration::from_millis(500)).is_err() {
        assert!(Instant::now() < deadline, "no change callback");
        n += 1;
        fs::write(dir.join("ultra.toml"), format!("# edit {n}\n")).unwrap();
    }
    assert_eq!(Arc::strong_count(&token), 2);
    drop(guard);
    // The callback (and everything it captured) is gone once drop returns.
    assert_eq!(Arc::strong_count(&token), 1);
    fs::remove_dir_all(root).unwrap();
}

/// inotify reports opens (Access events); macOS FSEvents never reports reads.
#[cfg(target_os = "linux")]
#[test]
fn reading_watched_files_does_not_call_back() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = scratch("reads");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("ultra.toml");
    fs::write(&file, "[sidebar]\nwidth = 300\n").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let guard = watch(&dir, move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    for _ in 0..5 {
        fs::read_to_string(&file).unwrap();
    }
    // Longer than the quiet period plus delivery, bounded.
    thread::sleep(QUIET * 4);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // The same watcher does report a write.
    fs::write(&file, "[sidebar]\nwidth = 310\n").unwrap();
    let deadline = Instant::now() + WAIT;
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "no change callback");
        thread::sleep(Duration::from_millis(20));
    }
    drop(guard);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn wakeups_are_capacity_one_and_nonblocking() {
    let (tx, rx) = mpsc::sync_channel(1);
    tx.try_send(()).unwrap();
    for _ in 0..10_000 {
        assert!(matches!(tx.try_send(()), Err(mpsc::TrySendError::Full(()))));
    }
    rx.recv().unwrap();
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    tx.try_send(()).unwrap();
}
