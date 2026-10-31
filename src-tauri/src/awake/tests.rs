use super::*;
use crate::status::Attention;

fn item(process: Process, activity: Activity) -> ItemState {
    ItemState {
        id: "item-1".into(),
        tool: "omp".into(),
        kind: crate::workspace::ItemKind::Agent,
        project_id: "/p".into(),
        cwd: "/p".into(),
        title: "OMP".into(),
        process: Some(process),
        document: None,
        exit_code: None,
        failure: None,
        activity,
        attention: activity.attention(),
        unread: false,
        conversation: None,
        model: None,
        started_at: 1,
        state_since: 1,
        pid: None,
        foreground_job: None,
        background_jobs: Vec::new(),
    }
}

#[test]
fn off_never_holds_sleep() {
    assert_eq!(reason(KeepAwake::Off, &[]), None);
    assert_eq!(
        reason(KeepAwake::Off, &[item(Process::Running, Activity::Working)]),
        None
    );
}

#[test]
fn working_counts_working_and_on_job_but_not_waiting_or_stopped_items() {
    assert_eq!(reason(KeepAwake::Working, &[]), None);
    for activity in [Activity::Working, Activity::OnJob] {
        assert_eq!(
            reason(KeepAwake::Working, &[item(Process::Running, activity)]),
            Some("an agent is working")
        );
        for process in [Process::Exited, Process::Failed] {
            assert_eq!(reason(KeepAwake::Working, &[item(process, activity)]), None);
        }
    }
    for activity in [
        Activity::Unknown,
        Activity::NeedsYou(Attention::Approval),
        Activity::YourTurn,
        Activity::Error,
    ] {
        assert_eq!(
            reason(KeepAwake::Working, &[item(Process::Running, activity)]),
            None
        );
    }
}

#[test]
fn running_counts_starting_and_running_regardless_of_activity() {
    assert_eq!(reason(KeepAwake::Running, &[]), None);
    for process in [Process::Starting, Process::Running] {
        assert_eq!(
            reason(KeepAwake::Running, &[item(process, Activity::Unknown)]),
            Some("items are running")
        );
    }
    for process in [Process::Exited, Process::Failed] {
        assert_eq!(
            reason(KeepAwake::Running, &[item(process, Activity::Working)]),
            None
        );
    }
    assert_eq!(
        reason(
            KeepAwake::Running,
            &[
                item(Process::Exited, Activity::Unknown),
                item(Process::Running, Activity::YourTurn)
            ]
        ),
        Some("items are running")
    );
}

#[test]
fn reason_changes_retain_a_hold_and_only_off_releases_it() {
    use std::{cell::Cell, rc::Rc};
    struct Resource(Rc<Cell<usize>>);
    impl Drop for Resource {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let dropped = Rc::new(Cell::new(0));
    let mut held = Hold::default();
    let now = Instant::now();
    assert_eq!(
        held.update(Some("an agent is working"), now, |_| Ok::<_, &str>(
            Resource(dropped.clone())
        )),
        None
    );
    // A replacement acquisition would fail, but changing text must not try it at all.
    assert_eq!(
        held.update(
            Some("items are running"),
            now,
            |_| -> Result<Resource, &str> { panic!("must retain the existing hold") }
        ),
        None
    );
    assert_eq!(dropped.get(), 0);
    assert!(held.value.is_some());
    assert_eq!(held.update(None, now, |_| Err("must not acquire")), None);
    assert_eq!(dropped.get(), 1);
    assert!(held.value.is_none());
}

#[test]
fn failed_acquisition_backs_off_and_off_resets_the_delay() {
    let mut held = Hold::<u32>::default();
    let now = Instant::now();
    assert_eq!(
        held.update(Some("working"), now, |_| Err("failed")),
        Some("failed")
    );
    assert!(held.value.is_none());
    assert_eq!(
        held.update(
            Some("running"),
            now + Duration::from_secs(29),
            |_| -> Result<u32, &str> { panic!("must not retry before 30 seconds") }
        ),
        None
    );
    // The boundary permits a retry, without repeating the consecutive-failure warning.
    assert_eq!(
        held.update(Some("working"), now + RETRY_DELAY, |_| Err("failed again")),
        None
    );
    assert_eq!(
        held.update(Some("working"), now + RETRY_DELAY * 2, |_| Ok::<_, &str>(
            42
        )),
        None
    );
    assert_eq!(held.value, Some(42));
    assert_eq!(
        held.update(None, now + RETRY_DELAY * 2, |_| Err("must not acquire")),
        None
    );
    assert_eq!(
        held.update(Some("working"), now + RETRY_DELAY * 2, |_| Err(
            "new failure"
        )),
        Some("new failure")
    );
    // Off -> on bypasses even a just-started failure backoff.
    assert_eq!(
        held.update(None, now + RETRY_DELAY * 2, |_| Err("must not acquire")),
        None
    );
    assert_eq!(
        held.update(Some("running"), now + RETRY_DELAY * 2, |_| Ok::<_, &str>(7)),
        None
    );
    assert_eq!(held.value, Some(7));
}
