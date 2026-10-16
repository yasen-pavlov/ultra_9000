//! `[power] keep_awake`: keep the computer from idle sleep while agents work or items run.
//! macOS holds an IOKit `PreventUserIdleSystemSleep` assertion (the display still sleeps);
//! Linux holds a logind `idle` inhibitor, which idle daemons such as hypridle honour.
use crate::{
    config::KeepAwake,
    status::{item_status, Activity, ItemStatus},
    workspace::{ItemState, Process},
};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
pub use linux::hold;
#[cfg(target_os = "macos")]
pub use macos::hold;

/// Why the computer stays awake now, or `None` to let it sleep. `Working` counts what the
/// agents themselves report (OMP), never inferred activity.
pub fn reason(mode: KeepAwake, items: &[ItemState]) -> Option<&'static str> {
    let any = |pred: fn(&ItemState) -> bool| items.iter().any(pred);
    match mode {
        KeepAwake::Off => None,
        KeepAwake::Working => any(|item| {
            matches!(
                item_status(item),
                ItemStatus::Activity(Activity::Working | Activity::OnJob)
            )
        })
        .then_some("an agent is working"),
        KeepAwake::Running => {
            any(|item| matches!(item.process, Process::Starting | Process::Running))
                .then_some("items are running")
        }
    }
}

const RETRY_DELAY: Duration = Duration::from_secs(30);

/// Enabled modes share one OS resource. Retain it across reason changes and back off
/// failed acquisitions, reporting only the first consecutive failure.
struct Hold<T> {
    value: Option<T>,
    reported: bool,
    last_attempt: Option<Instant>,
}
impl<T> Default for Hold<T> {
    fn default() -> Self {
        Self {
            value: None,
            reported: false,
            last_attempt: None,
        }
    }
}
impl<T> Hold<T> {
    fn update<E>(
        &mut self,
        reason: Option<&'static str>,
        now: Instant,
        acquire: impl FnOnce(&'static str) -> Result<T, E>,
    ) -> Option<E> {
        let Some(reason) = reason else {
            self.value = None;
            self.reported = false;
            self.last_attempt = None;
            return None;
        };
        if self.value.is_some()
            || self
                .last_attempt
                .is_some_and(|last| now.duration_since(last) < RETRY_DELAY)
        {
            return None;
        }
        match acquire(reason) {
            Ok(value) => {
                self.value = Some(value);
                self.reported = false;
                self.last_attempt = None;
                None
            }
            Err(error) => {
                let report = !self.reported;
                self.last_attempt = Some(now);
                self.reported = true;
                report.then_some(error)
            }
        }
    }
}

#[cfg(test)]
mod tests;
