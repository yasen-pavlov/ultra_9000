use super::*;
use crate::test_support::PrivateBus;
use std::{
    fs::File,
    io::Read,
    os::fd::{FromRawFd, OwnedFd as StdOwnedFd},
    sync::{
        atomic::{AtomicBool, Ordering},
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
fn server(bus: &PrivateBus) -> (Connection, mpsc::Receiver<Call>, Arc<AtomicBool>) {
    let (calls, received) = mpsc::channel();
    let fail = Arc::new(AtomicBool::new(false));
    let connection = Builder::address(bus.address.as_str())
        .unwrap()
        .name("org.freedesktop.login1")
        .unwrap()
        .serve_at(
            "/org/freedesktop/login1",
            Manager {
                calls,
                fail: fail.clone(),
            },
        )
        .unwrap()
        .build()
        .unwrap();
    (connection, received, fail)
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

#[test]
fn private_logind_holds_the_returned_fd_across_modes_and_releases_it_on_off() {
    let Some(private) = PrivateBus::start() else {
        return;
    };
    let (_server, calls, _) = server(&private);
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
    assert!(
        calls.recv_timeout(Duration::from_millis(100)).is_err(),
        "a mode change must retain the original fd"
    );
    request(None, &enabled, |reason| worker.send(reason).unwrap());
    released(&mut peer);
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
    hold(None);
}

#[test]
fn a_failed_logind_call_backs_off_until_off_then_on_retries() {
    let Some(private) = PrivateBus::start() else {
        return;
    };
    let (_server, calls, fail) = server(&private);
    fail.store(true, Ordering::Relaxed);
    let address = private.address.clone();
    let worker = worker(move || Builder::address(address.as_str())?.build()).unwrap();
    worker.send(Some("working")).unwrap();
    assert!(calls
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .peer
        .is_none());
    worker.send(Some("running")).unwrap();
    assert!(
        calls.recv_timeout(Duration::from_millis(100)).is_err(),
        "failed requests must back off, even across mode changes"
    );
    fail.store(false, Ordering::Relaxed);
    worker.send(None).unwrap();
    worker.send(Some("running")).unwrap();
    let mut peer = calls
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .peer
        .unwrap();
    worker.send(None).unwrap();
    released(&mut peer);
}
