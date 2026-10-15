use std::{
    cell::Cell,
    sync::{mpsc, LazyLock},
};
use zbus::{blocking::Connection, zvariant::OwnedFd};

/// Requests for the inhibitor worker: system-bus calls never run on the main thread.
static WORKER: LazyLock<Option<mpsc::Sender<Option<&'static str>>>> = LazyLock::new(|| {
    let (sender, requests) = mpsc::channel();
    std::thread::Builder::new()
        .name("u9k-keep-awake".into())
        .spawn(move || run(&requests))
        .inspect_err(|e| log::warn!("keep awake unavailable: {e}"))
        .ok()
        .map(|_| sender)
});
thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
}

/// Main thread. Enabled publications allow retries after an acquisition failed.
pub fn hold(reason: Option<&'static str>) {
    if !ENABLED.replace(reason.is_some()) && reason.is_none() {
        return;
    }
    if let Some(worker) = &*WORKER {
        let _ = worker.send(reason);
    }
}

fn run(requests: &mpsc::Receiver<Option<&'static str>>) {
    let mut bus = None;
    let mut held = super::Hold::default();
    while let Ok(mut reason) = requests.recv() {
        // Only the latest request matters.
        while let Ok(next) = requests.try_recv() {
            reason = next;
        }
        if let Some(error) = held.update(reason, std::time::Instant::now(), |_| {
            let result = inhibit(&mut bus);
            if result.is_err() {
                bus = None;
            }
            result
        }) {
            log::warn!("keep awake: logind Inhibit failed: {error}");
        }
    }
}

/// logind holds the inhibitor until the returned descriptor closes (or the app exits).
fn inhibit(bus: &mut Option<Connection>) -> zbus::Result<OwnedFd> {
    let bus = match bus {
        Some(bus) => bus,
        None => bus.insert(Connection::system()?),
    };
    let reply = bus.call_method(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        Some("org.freedesktop.login1.Manager"),
        "Inhibit",
        &("idle", "Ultra 9000", "Ultra 9000 keep awake", "block"),
    )?;
    reply.body().deserialize()
}
