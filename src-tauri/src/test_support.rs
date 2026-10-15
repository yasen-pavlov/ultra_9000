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
    _directory: tempfile::TempDir,
}
impl PrivateBus {
    pub(crate) fn start() -> Option<Self> {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("busconfig.xml");
        std::fs::write(
            &config,
            r#"<busconfig>
<type>session</type>
<listen>unix:tmpdir=/tmp</listen>
<auth>EXTERNAL</auth>
<policy context="default">
  <allow send_destination="*"/>
  <allow receive_sender="*"/>
  <allow own="*"/>
</policy>
</busconfig>"#,
        )
        .unwrap();
        // No service directories: even an unowned destination can never activate
        // an installed desktop daemon. Do not pass any desktop/bus environment on.
        let mut command = Command::new("dbus-daemon");
        command
            .arg("--config-file")
            .arg(&config)
            .args(["--nofork", "--print-address=1"]);
        for (key, _) in std::env::vars_os() {
            if key == "WAYLAND_DISPLAY"
                || key == "DISPLAY"
                || key.to_string_lossy().starts_with("DBUS_")
            {
                command.env_remove(key);
            }
        }
        let child = match command
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                assert!(
                    std::env::var_os("U9K_REQUIRE_GUI_TESTS").is_none(),
                    "dbus-daemon is required for the CI private-bus tests"
                );
                eprintln!("skipping private D-Bus test: dbus-daemon is not installed");
                return None;
            }
            Err(error) => panic!("starting private dbus-daemon: {error}"),
        };
        let mut bus = Self {
            address: String::new(),
            child,
            _directory: directory,
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
