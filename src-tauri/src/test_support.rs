//! Test-owned resources only; never connect to the desktop session or system bus.
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

pub(crate) struct PrivateBus {
    pub(crate) address: String,
    child: Child,
}
impl PrivateBus {
    pub(crate) fn start() -> Option<Self> {
        let child = match Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("skipping private D-Bus test: dbus-daemon is not installed");
                return None;
            }
            Err(error) => panic!("starting private dbus-daemon: {error}"),
        };
        let mut bus = Self {
            address: String::new(),
            child,
        };
        let stdout = bus.child.stdout.take().unwrap();
        let (sender, ready) = mpsc::channel();
        std::thread::spawn(move || {
            let mut address = String::new();
            BufReader::new(stdout).read_line(&mut address).unwrap();
            let _ = sender.send(address);
        });
        bus.address = ready
            .recv_timeout(Duration::from_secs(5))
            .expect("private bus startup timed out")
            .trim()
            .to_owned();
        assert!(!bus.address.is_empty(), "private bus returned no address");
        Some(bus)
    }
}
impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
