use super::*;
use crate::test_support::PrivateBus;
use std::{
    fs::File,
    io::Read,
    os::fd::{FromRawFd, OwnedFd as StdOwnedFd},
    os::unix::fs::MetadataExt,
    process::{Child, Command},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use zbus::blocking::connection::Builder;

struct Call {
    arguments: (String, String, String, String),
    peer: Option<File>,
}
struct Manager {
    calls: mpsc::Sender<Call>,
    fail: Arc<AtomicBool>,
    count: Arc<AtomicUsize>,
}
fn pipe() -> (File, StdOwnedFd) {
    let mut fds = [-1; 2];
    // SAFETY: pipe2 initializes two distinct owned fds; neither is used before success.
    assert_eq!(
        unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    // SAFETY: each fresh fd is adopted exactly once.
    unsafe { (File::from_raw_fd(fds[0]), StdOwnedFd::from_raw_fd(fds[1])) }
}
#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl Manager {
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::fdo::Result<OwnedFd> {
        self.count.fetch_add(1, Ordering::SeqCst);
        let arguments = (what.into(), who.into(), why.into(), mode.into());
        if self.fail.load(Ordering::Relaxed) {
            self.calls
                .send(Call {
                    arguments,
                    peer: None,
                })
                .unwrap();
            return Err(zbus::fdo::Error::Failed(
                "test logind denied inhibitor".into(),
            ));
        }
        let (peer, fd) = pipe();
        self.calls
            .send(Call {
                arguments,
                peer: Some(peer),
            })
            .unwrap();
        Ok(fd.into())
    }
}
fn server(
    bus: &PrivateBus,
) -> (
    Connection,
    mpsc::Receiver<Call>,
    Arc<AtomicBool>,
    Arc<AtomicUsize>,
) {
    let (calls, received) = mpsc::channel();
    let fail = Arc::new(AtomicBool::new(false));
    let count = Arc::new(AtomicUsize::new(0));
    let connection = Builder::address(bus.address.as_str())
        .unwrap()
        .name("org.freedesktop.login1")
        .unwrap()
        .serve_at(
            "/org/freedesktop/login1",
            Manager {
                calls,
                fail: fail.clone(),
                count: count.clone(),
            },
        )
        .unwrap()
        .build()
        .unwrap();
    (connection, received, fail, count)
}
fn released(peer: &mut File) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match peer.read(&mut [0]) {
            Ok(0) => return,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            other => panic!("unexpected inhibitor pipe read: {other:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "inhibitor descriptor was not released"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

struct Sleeper(Child);
impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn private_logind_holds_the_returned_fd_across_modes_and_releases_it_on_off() {
    let Some(private) = PrivateBus::start() else {
        return;
    };
    let (_server, calls, _, count) = server(&private);
    let address = private.address.clone();
    let worker = worker(move || Builder::address(address.as_str())?.build()).unwrap();
    let enabled = Cell::new(false);
    request(None, &enabled, |_| {
        panic!("off must not start a worker request")
    });
    request(Some("working"), &enabled, |reason| {
        worker.send(reason).unwrap()
    });
    let call = calls.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        call.arguments,
        (
            "idle".into(),
            "Ultra 9000".into(),
            "Ultra 9000 keep awake".into(),
            "block".into()
        )
    );
    let mut peer = call.peer.unwrap();
    assert_eq!(
        peer.read(&mut [0]).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    request(Some("running"), &enabled, |reason| {
        worker.send(reason).unwrap()
    });
    // The release is a FIFO barrier: every preceding request has completed once EOF
    // arrives, so an exact count proves the mode change did not acquire again.
    request(None, &enabled, |reason| worker.send(reason).unwrap());
    released(&mut peer);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(calls.try_recv().is_err());
    request(None, &enabled, |_| {
        panic!("a second off request is redundant")
    });
    // Reacquire using the cached system connection, then worker shutdown releases it too.
    request(Some("running"), &enabled, |reason| {
        worker.send(reason).unwrap()
    });
    let mut peer = calls
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .peer
        .unwrap();
    drop(worker);
    released(&mut peer);
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert!(calls.try_recv().is_err());
}

#[test]
fn a_failed_logind_call_backs_off_until_off_then_on_retries() {
    let Some(private) = PrivateBus::start() else {
        return;
    };
    let (_server, calls, fail, count) = server(&private);
    fail.store(true, Ordering::Relaxed);
    let address = private.address.clone();
    let (worker, requests) = mpsc::channel();
    let done =
        std::thread::spawn(move || run(&requests, || Builder::address(address.as_str())?.build()));
    worker.send(Some("working")).unwrap();
    assert!(calls
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .peer
        .is_none());
    worker.send(Some("running")).unwrap();
    // Queue the potentially suppressed request before the off/on edge. Joining the
    // FIFO worker below makes the negative assertion independent of scheduling.
    fail.store(false, Ordering::Relaxed);
    worker.send(None).unwrap();
    worker.send(Some("running")).unwrap();
    worker.send(None).unwrap();
    drop(worker);
    done.join().unwrap();
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "only the initial failure and off/on retry may call logind"
    );
    let remaining: Vec<_> = calls.try_iter().collect();
    assert_eq!(remaining.len(), 1);
    let mut peer = remaining.into_iter().next().unwrap().peer.unwrap();
    released(&mut peer);
}

#[test]
fn a_child_started_while_held_cannot_extend_the_inhibitor_after_off() {
    let Some(private) = PrivateBus::start() else {
        return;
    };
    let (_server, calls, _, count) = server(&private);
    let mut bus = None;
    let mut connect = || Builder::address(private.address.as_str())?.build();
    let enabled = Cell::new(false);
    let mut held = crate::awake::Hold::default();
    request(Some("working"), &enabled, |reason| {
        assert!(held
            .update(reason, Instant::now(), |_| inhibit(&mut bus, &mut connect))
            .is_none());
    });
    // This verifies the acquired end state, not the transient recvmsg -> fcntl
    // window: OwnedFd deserialization already duplicates with CLOEXEC. The pipe
    // scan additionally catches any non-CLOEXEC copies retained by future zbus versions.
    assert!(held.value.is_some());
    let mut peer = calls
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .peer
        .unwrap();
    let fd = held.value.as_ref().unwrap().as_raw_fd();
    // SAFETY: the held descriptor remains owned and open throughout this check.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
    let identity = peer.metadata().unwrap();
    let mut matching = 0;
    for entry in std::fs::read_dir("/proc/self/fd").unwrap() {
        let entry = entry.unwrap();
        let Ok(metadata) = std::fs::metadata(entry.path()) else {
            continue;
        };
        if metadata.dev() != identity.dev() || metadata.ino() != identity.ino() {
            continue;
        }
        let fd: libc::c_int = entry.file_name().to_str().unwrap().parse().unwrap();
        // SAFETY: F_GETFD only reads flags. A concurrently closed transient copy
        // may return EBADF; the peer and held handle remain open.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 {
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EBADF)
            );
            continue;
        }
        assert_ne!(
            flags & libc::FD_CLOEXEC,
            0,
            "inhibitor pipe fd {fd} is inheritable"
        );
        matching += 1;
    }
    assert!(
        matching >= 2,
        "the peer and held inhibitor must both be checked"
    );
    let mut child = Sleeper(Command::new("sleep").arg("30").spawn().unwrap());
    assert!(child.0.try_wait().unwrap().is_none());
    request(None, &enabled, |reason| {
        assert!(held
            .update(reason, Instant::now(), |_| -> zbus::Result<OwnedFd> {
                panic!("off must release, not acquire")
            })
            .is_none());
    });
    assert!(held.value.is_none());
    released(&mut peer);
    assert!(
        child.0.try_wait().unwrap().is_none(),
        "release must not wait for the child to exit"
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
}
