use objc2_core_foundation::CFString;
use objc2_io_kit::{
    kIOPMAssertionLevelOn, IOPMAssertionCreateWithName, IOPMAssertionID, IOPMAssertionRelease,
};
use std::cell::RefCell;

struct Assertion(IOPMAssertionID);
impl Drop for Assertion {
    fn drop(&mut self) {
        IOPMAssertionRelease(self.0);
    }
}
thread_local! {
    static HELD: RefCell<super::Hold<Assertion>> = RefCell::default();
}

/// Main thread. A reason change does not replace a valid idle-sleep assertion.
pub fn hold(reason: Option<&'static str>) {
    HELD.with_borrow_mut(|held| {
        if let Some(error) = held.update(reason, std::time::Instant::now(), create) {
            log::warn!("keep awake: IOPMAssertionCreateWithName returned {error:#x}");
        }
    });
}

fn create(_reason: &'static str) -> Result<Assertion, i32> {
    let kind = CFString::from_static_str("PreventUserIdleSystemSleep");
    let name = CFString::from_static_str("Ultra 9000 keep awake");
    let mut id: IOPMAssertionID = 0;
    // SAFETY: both strings are live CFStrings and `id` is a valid out pointer.
    let result = unsafe {
        IOPMAssertionCreateWithName(Some(&kind), kIOPMAssertionLevelOn, Some(&name), &mut id)
    };
    if result == 0 {
        Ok(Assertion(id))
    } else {
        Err(result)
    }
}
