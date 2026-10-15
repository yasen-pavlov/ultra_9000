use super::*;
use std::{
    process::Command,
    time::{Duration, Instant},
};

fn listed() -> bool {
    let output = Command::new("/usr/bin/pmset")
        .args(["-g", "assertions"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "pmset failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let process = format!("pid {}(", std::process::id());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .any(|line| line.contains(&process) && line.contains("Ultra 9000 keep awake"))
}

#[test]
fn real_idle_sleep_assertion_is_listed_retained_across_modes_and_released() {
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            hold(None);
        }
    }
    let _release = Release;
    hold(None);
    hold(Some("an agent is working"));
    let id = HELD.with_borrow(|held| {
        held.value
            .as_ref()
            .expect("IOKit assertion creation failed")
            .0
    });
    assert!(
        listed(),
        "pmset must list this process's real idle-sleep assertion"
    );
    hold(Some("items are running"));
    assert_eq!(HELD.with_borrow(|held| held.value.as_ref().unwrap().0), id);
    hold(None);
    assert!(HELD.with_borrow(|held| held.value.is_none()));
    let deadline = Instant::now() + Duration::from_secs(5);
    while listed() {
        assert!(
            Instant::now() < deadline,
            "released assertion remains in pmset"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
