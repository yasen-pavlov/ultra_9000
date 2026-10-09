use gtk::glib;
use std::{
    future::Future,
    sync::{mpsc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};
static MAIN_CONTEXT: Mutex<()> = Mutex::new(());
pub(crate) struct Main {
    context: glib::MainContext,
    _exclusive: MutexGuard<'static, ()>,
}
impl Main {
    /// Become the main thread. Tasks queued before (by other tests) run here first, while
    /// this thread has no workspace yet.
    pub(crate) fn acquire() -> Self {
        let main = Self {
            _exclusive: MAIN_CONTEXT.lock().unwrap_or_else(PoisonError::into_inner),
            context: glib::MainContext::default(),
        };
        main.idle();
        main
    }
    /// Run every main-thread task queued so far.
    pub(crate) fn idle(&self) {
        while self.context.iteration(false) {}
    }
    /// Run idle until `done` holds, failing after a generous deadline.
    pub(crate) fn until(&self, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.idle();
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    /// Drive an async command to completion, running its main-thread parts here.
    pub(crate) fn run<T: Send + 'static>(
        &self,
        command: impl Future<Output = T> + Send + 'static,
    ) -> T {
        let (tx, rx) = mpsc::channel();
        tauri::async_runtime::spawn(async move {
            let _ = tx.send(command.await);
        });
        let mut result = None;
        self.until("the command", || {
            result = result.take().or_else(|| rx.try_recv().ok());
            result.is_some()
        });
        // Effects the worker posted before replying are applied too.
        self.idle();
        result.unwrap()
    }
}
pub(crate) fn mock_app(main_window: bool) -> tauri::App<tauri::test::MockRuntime> {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    if main_window {
        tauri::WebviewWindowBuilder::new(
            &app,
            crate::lifecycle::MAIN_WINDOW,
            tauri::WebviewUrl::default(),
        )
        .build()
        .unwrap();
    }
    app
}
