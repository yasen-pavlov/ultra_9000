use std::{
    cell::Cell,
    os::fd::AsRawFd,
    sync::{mpsc, LazyLock},
};
use zbus::{blocking::Connection, zvariant::OwnedFd};

/// Requests for the inhibitor worker: system-bus calls never run on the main thread.
static WORKER: LazyLock<Option<mpsc::Sender<Option<&str>>>> =
    LazyLock::new(|| worker(Connection::system));

fn worker(
    connect: impl FnMut() -> zbus::Result<Connection> + Send + 'static,
) -> Option<mpsc::Sender<Option<&'static str>>> {
    let (sender, requests) = mpsc::channel();
    std::thread::Builder::new()
        .name("u9k-keep-awake".into())
        .spawn(move || run(&requests, connect))
        .inspect_err(|e| log::warn!("keep awake unavailable: {e}"))
        .ok()
        .map(|_| sender)
}
thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
}

/// Main thread. Enabled publications allow retries after an acquisition failed.
pub fn hold(reason: Option<&'static str>) {
    ENABLED.with(|enabled| {
        request(reason, enabled, |reason| {
            if let Some(worker) = &*WORKER {
                let _ = worker.send(reason);
            }
        })
    });
}

fn request(
    reason: Option<&'static str>,
    enabled: &Cell<bool>,
    send: impl FnOnce(Option<&'static str>),
) {
    if !enabled.replace(reason.is_some()) && reason.is_none() {
        return;
    }
    send(reason);
}

fn run(
    requests: &mpsc::Receiver<Option<&'static str>>,
    mut connect: impl FnMut() -> zbus::Result<Connection>,
) {
    let mut bus = None;
    let mut held = super::Hold::default();
    while let Ok(reason) = requests.recv() {
        if let Some(error) = held.update(reason, std::time::Instant::now(), |_| {
            let result = inhibit(&mut bus, &mut connect);
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
fn inhibit(
    bus: &mut Option<Connection>,
    connect: &mut impl FnMut() -> zbus::Result<Connection>,
) -> zbus::Result<OwnedFd> {
    let bus = match bus {
        Some(bus) => bus,
        None => bus.insert(connect()?),
    };
    let reply = bus.call_method(
        Some("org.freedesktop.login1"),
        "/org/freedesktop/login1",
        Some("org.freedesktop.login1.Manager"),
        "Inhibit",
        &("idle", "Ultra 9000", "Ultra 9000 keep awake", "block"),
    )?;
    // zbus may retain message copies of the SCM_RIGHTS descriptors. Mark those
    // originals too, not just the owned handle deserialized below.
    for fd in reply.data().fds() {
        close_on_exec(fd.as_raw_fd())?;
    }
    let fd: OwnedFd = reply.body().deserialize()?;
    close_on_exec(fd.as_raw_fd())?;
    Ok(fd)
}

fn close_on_exec(fd: std::os::fd::RawFd) -> std::io::Result<()> {
    // SAFETY: the caller owns a live fd; these commands only read/update its fd flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if flags & libc::FD_CLOEXEC == 0
        && unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
