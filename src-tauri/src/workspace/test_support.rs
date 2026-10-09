use super::*;
use embed::ForegroundJob;

/// A launch the workspace asked the host to prepare off-main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prepared {
    pub(crate) id: String,
    pub(crate) tool: String,
    pub(crate) cwd: String,
    pub(crate) args: Vec<String>,
}
struct Session {
    state: TerminalState,
    shown: bool,
    job: Option<ForegroundJob>,
}
pub(crate) struct FakeHost {
    /// Every published workspace snapshot, in order.
    pub(crate) published: RefCell<Vec<WorkspaceState>>,
    /// Every other app event (`conversations-changed`, `settings-state`), in order.
    pub(crate) events: RefCell<Vec<(String, serde_json::Value)>>,
    pub(crate) prepared: RefCell<Vec<Prepared>>,
    /// Session effects in order: `create 1`, `stop 1`, `remove 1`, `hide 1`,
    /// `viewport 1 true`, `focus 1`, `reload`, `focus-sidebar`.
    pub(crate) calls: RefCell<Vec<String>>,
    pub(crate) notifications: RefCell<Vec<(String, String)>>,
    pub(crate) pin_probes: RefCell<Vec<Vec<PathBuf>>>,
    /// Next `create` fails with this message.
    pub(crate) create_error: RefCell<Option<String>>,
    /// `reload_config` fails with this message.
    pub(crate) reload_error: RefCell<Option<String>>,
    /// Terminal title a new session reports first.
    pub(crate) initial_title: RefCell<String>,
    pub(crate) presentable: Cell<bool>,
    pub(crate) focused: Cell<bool>,
    sessions: RefCell<BTreeMap<SessionKey, Session>>,
    next_key: Cell<SessionKey>,
}
/// Installs a fresh workspace on the current thread, with an attached terminal host, a
/// presentable focused window and `/` as default directory. Replaces any previous one.
pub(crate) fn install() -> Rc<FakeHost> {
    let host = Rc::new(FakeHost {
        published: RefCell::default(),
        events: RefCell::default(),
        prepared: RefCell::default(),
        calls: RefCell::default(),
        notifications: RefCell::default(),
        pin_probes: RefCell::default(),
        create_error: RefCell::default(),
        reload_error: RefCell::default(),
        initial_title: RefCell::default(),
        presentable: Cell::new(true),
        focused: Cell::new(true),
        sessions: RefCell::default(),
        next_key: Cell::new(1),
    });
    let mut workspace = Workspace::new(host.clone());
    workspace.host_error = None;
    workspace.default_cwd = "/".into();
    WORKSPACE.with(|cell| *cell.borrow_mut() = Some(workspace));
    HOST_EVENTS.with(|events| *events.borrow_mut() = HostQueue::default());
    host
}
impl FakeHost {
    /// Completes a prepared launch: the session starts running.
    pub(crate) fn complete(&self, id: &str) -> SessionKey {
        let cwd = self.prepared_launch(id).cwd;
        finish_launch(
            id,
            Ok(Launch {
                command: None,
                executable: Some("/bin/sh".into()),
                working_directory: cwd.into(),
                env: vec![],
            }),
        );
        self.key(id).expect("launch did not start a session")
    }
    /// Fails a prepared launch (e.g. the command is not on PATH).
    pub(crate) fn fail(&self, id: &str, message: &str) {
        self.prepared_launch(id);
        finish_launch(id, Err(message.into()));
    }
    fn prepared_launch(&self, id: &str) -> Prepared {
        self.prepared
            .borrow()
            .iter()
            .rev()
            .find(|p| p.id == id)
            .cloned()
            .expect("item was never prepared")
    }
    /// Session key of a live item.
    pub(crate) fn key(&self, id: &str) -> Option<SessionKey> {
        with(|w| {
            Ok(w.records
                .iter()
                .find(|r| r.state.id == id)
                .and_then(|r| r.key))
        })
        .unwrap()
    }
    /// The process of a session exits; the host reports it like the terminal does.
    pub(crate) fn exit(&self, key: SessionKey, code: Option<i32>) {
        let state = self.update(key, |s| {
            s.running = false;
            s.exit_code = code;
        });
        deliver(key, embed::HostEvent::State(&state));
    }
    /// The session's terminal title changes.
    pub(crate) fn retitle(&self, key: SessionKey, title: &str) {
        let state = self.update(key, |s| s.title = title.into());
        deliver(key, embed::HostEvent::State(&state));
    }
    /// OMP emits an OSC 777 event (JSON body) on the session.
    pub(crate) fn omp_event(&self, key: SessionKey, json: &str) {
        let payload = format!("notify;warp://cli-agent;{json}");
        deliver(key, embed::HostEvent::Notify(&payload));
    }
    /// A process group (`pgid`, `name`) holds the session's terminal; `None` is the prompt.
    pub(crate) fn set_job(&self, key: SessionKey, job: Option<(i32, &str)>) {
        if let Some(session) = self.sessions.borrow_mut().get_mut(&key) {
            session.job = job.map(|(pgid, name)| ForegroundJob {
                pgid,
                name: Some(name.into()),
            });
        }
    }
    /// Recorded calls of one kind (`stop`, `remove`, `hide`, ...), as session keys.
    pub(crate) fn calls_of(&self, kind: &str) -> Vec<SessionKey> {
        self.calls
            .borrow()
            .iter()
            .filter_map(|c| c.strip_prefix(kind)?.strip_prefix(' '))
            .filter_map(|rest| rest.split(' ').next()?.parse().ok())
            .collect()
    }
    pub(crate) fn last_published(&self) -> WorkspaceState {
        self.published
            .borrow()
            .last()
            .cloned()
            .expect("nothing was published")
    }
    fn update(&self, key: SessionKey, f: impl FnOnce(&mut TerminalState)) -> TerminalState {
        let mut sessions = self.sessions.borrow_mut();
        let session = sessions.get_mut(&key).expect("no such session");
        f(&mut session.state);
        session.state.clone()
    }
    fn log(&self, call: String) {
        self.calls.borrow_mut().push(call);
    }
}
/// Delivers a host event through the terminal host's listener, then flushes it like the
/// main loop does (the scheduled main-loop flush only finds an empty queue later).
pub(crate) fn deliver(key: SessionKey, event: embed::HostEvent<'_>) {
    host_event(key, event);
    flush_host_events();
}
impl Host for FakeHost {
    fn publish(&self, state: WorkspaceState) {
        self.published.borrow_mut().push(state);
    }
    fn emit(&self, event: &str, payload: serde_json::Value) {
        self.events.borrow_mut().push((event.into(), payload));
    }
    fn prepare(&self, id: String, spec: ToolSpec, cwd: String, args: Vec<String>) {
        self.prepared.borrow_mut().push(Prepared {
            id,
            tool: spec.id,
            cwd,
            args,
        });
    }
    fn create(&self, launch: Launch) -> Result<SessionKey, String> {
        if let Some(error) = self.create_error.borrow_mut().take() {
            return Err(error);
        }
        let key = self.next_key.get();
        self.next_key.set(key + 1);
        let state = TerminalState {
            title: self.initial_title.borrow().clone(),
            shell: launch.executable.unwrap_or_else(|| "/bin/sh".into()),
            pid: Some(1000 + key as u32),
            running: true,
            exit_code: None,
            rows: 24,
            cols: 80,
            focused: false,
        };
        self.sessions.borrow_mut().insert(
            key,
            Session {
                state,
                shown: false,
                job: None,
            },
        );
        self.log(format!("create {key}"));
        Ok(key)
    }
    fn state(&self, key: SessionKey) -> Option<TerminalState> {
        self.sessions.borrow().get(&key).map(|s| s.state.clone())
    }
    fn foreground_job(&self, key: SessionKey) -> Option<ForegroundJob> {
        self.sessions.borrow().get(&key)?.job.clone()
    }
    fn stop(&self, key: SessionKey) -> Result<(), String> {
        if !self.sessions.borrow().contains_key(&key) {
            return Err("terminal session does not exist".into());
        }
        self.log(format!("stop {key}"));
        Ok(())
    }
    fn remove(&self, key: SessionKey) -> Result<(), String> {
        let mut sessions = self.sessions.borrow_mut();
        match sessions.get(&key) {
            None => return Err("terminal session does not exist".into()),
            Some(s) if s.state.running => return Err("terminal session is still running".into()),
            Some(_) => sessions.remove(&key),
        };
        drop(sessions);
        self.log(format!("remove {key}"));
        Ok(())
    }
    fn hide(&self, key: SessionKey) {
        if let Some(session) = self.sessions.borrow_mut().get_mut(&key) {
            session.shown = false;
        }
        self.log(format!("hide {key}"));
    }
    fn is_shown(&self, key: SessionKey) -> bool {
        self.sessions.borrow().get(&key).is_some_and(|s| s.shown)
    }
    fn set_viewport(
        &self,
        key: SessionKey,
        rect: ViewportRect,
        visible: bool,
    ) -> Result<(), String> {
        let mut sessions = self.sessions.borrow_mut();
        let session = sessions
            .get_mut(&key)
            .ok_or("terminal session does not exist")?;
        session.shown = visible && rect.width > 0. && rect.height > 0.;
        drop(sessions);
        self.log(format!("viewport {key} {visible}"));
        Ok(())
    }
    fn focus(&self, key: SessionKey) -> bool {
        let shown = self.is_shown(key);
        if shown {
            self.log(format!("focus {key}"));
        }
        shown
    }
    fn reload_config(&self, _toml: &str) -> Result<(), String> {
        self.log("reload".into());
        self.reload_error.borrow().clone().map_or(Ok(()), Err)
    }
    fn focus_sidebar(&self) {
        self.log("focus-sidebar".into());
    }
    fn window_presentable(&self) -> bool {
        self.presentable.get()
    }
    fn window_focused(&self) -> bool {
        self.focused.get()
    }
    fn notify(&self, summary: &str, body: &str) {
        self.notifications
            .borrow_mut()
            .push((summary.into(), body.into()));
    }
    fn probe_pinned(&self, pinned: Vec<PathBuf>) {
        self.pin_probes.borrow_mut().push(pinned);
    }
}
