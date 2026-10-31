use super::*;
use embed::ForegroundJob;

/// A launch the workspace asked the host to prepare off-main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prepared {
    pub(crate) id: String,
    pub(crate) tool: String,
    pub(crate) cwd: String,
    pub(crate) args: Vec<String>,
    /// The launch got status-link credentials.
    pub(crate) linked: bool,
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
    /// Validated paths sent to the system opener, pending completion.
    pub(crate) system_files: RefCell<VecDeque<String>>,
    /// Validated launchers sent to the file manager, pending completion.
    pub(crate) revealed_files: RefCell<VecDeque<String>>,
    /// Session effects in order: `create 1`, `stop 1`, `remove 1`, `hide 1`,
    /// `viewport 1 true`, `focus 1`, `reload`, `focus-sidebar`.
    pub(crate) calls: RefCell<Vec<String>>,
    pub(crate) notifications: RefCell<Vec<(String, String, String)>>,
    pub(crate) keep_awake: RefCell<Vec<Option<&'static str>>>,
    pub(crate) pin_probes: RefCell<Vec<Vec<PathBuf>>>,
    /// Status-link token effects in order: `link item-1`, `unlink item-1`.
    pub(crate) links: RefCell<Vec<String>>,
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
    file_requests: RefCell<VecDeque<(FileOrigin, String)>>,
}
/// Installs a fresh workspace on the current thread, with an attached terminal host, a
/// presentable focused window and `/` as default directory. Replaces any previous one.
pub(crate) fn install() -> Rc<FakeHost> {
    let host = Rc::new(FakeHost {
        published: RefCell::default(),
        events: RefCell::default(),
        prepared: RefCell::default(),
        system_files: RefCell::default(),
        revealed_files: RefCell::default(),
        calls: RefCell::default(),
        notifications: RefCell::default(),
        keep_awake: RefCell::default(),
        links: RefCell::default(),
        pin_probes: RefCell::default(),
        create_error: RefCell::default(),
        reload_error: RefCell::default(),
        initial_title: RefCell::default(),
        presentable: Cell::new(true),
        focused: Cell::new(true),
        sessions: RefCell::default(),
        next_key: Cell::new(1),
        file_requests: RefCell::default(),
    });
    let mut workspace = Workspace::new(host.clone());
    workspace.host_error = None;
    workspace.default_cwd = "/".into();
    WORKSPACE.with(|cell| *cell.borrow_mut() = Some(workspace));
    HOST_EVENTS.with(|events| *events.borrow_mut() = HostQueue::default());
    host
}
impl FakeHost {
    /// Number of file clicks waiting for their worker to finish.
    pub(crate) fn pending_files(&self) -> usize {
        self.file_requests.borrow().len()
    }
    /// Completes the next file-validation worker using the real URI/filesystem validation.
    pub(crate) fn complete_file(&self) {
        let (origin, uri) = self
            .file_requests
            .borrow_mut()
            .pop_front()
            .expect("no file validation was requested");
        let target = file_links::validate(&uri, &origin.cwd);
        finish_file_link(origin, target);
    }
    /// Completes the desktop opener without invoking a real application.
    pub(crate) fn complete_system_file(&self, result: Result<(), String>) {
        self.system_files
            .borrow_mut()
            .pop_front()
            .expect("no system file open was requested");
        finish_system_file(result);
    }
    pub(crate) fn complete_reveal_file(&self, result: Result<(), String>) {
        self.revealed_files
            .borrow_mut()
            .pop_front()
            .expect("no file reveal was requested");
        finish_system_file(result);
    }
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
    /// The item's OMP sends one status-link line (`json`) on connection 1.
    pub(crate) fn omp(&self, id: &str, json: &str) {
        self.omp_on(id, 1, json);
    }
    /// The item's OMP sends one status-link line on the numbered connection.
    pub(crate) fn omp_on(&self, id: &str, connection: u64, json: &str) {
        let message = omp_link::parse(json.as_bytes()).expect("not a status-link message");
        omp_link(Delivery {
            item: id.into(),
            connection,
            message,
        });
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
/// One foreground-job poll, as the app's 1 s timer runs it.
pub(crate) fn poll_foreground_jobs() {
    poll_jobs();
}
impl Host for FakeHost {
    fn publish(&self, state: WorkspaceState) {
        self.published.borrow_mut().push(state);
    }
    fn emit(&self, event: &str, payload: serde_json::Value) {
        self.events.borrow_mut().push((event.into(), payload));
    }
    fn prepare(
        &self,
        id: String,
        spec: ToolSpec,
        cwd: String,
        args: Vec<String>,
        link: Option<Credentials>,
    ) {
        self.prepared.borrow_mut().push(Prepared {
            id,
            tool: spec.id,
            cwd,
            args,
            linked: link.is_some(),
        });
    }
    fn prepare_file(&self, origin: FileOrigin, uri: String) {
        self.file_requests.borrow_mut().push_back((origin, uri));
    }
    fn open_file(&self, path: String) {
        self.system_files.borrow_mut().push_back(path);
    }
    fn reveal_file(&self, path: String) {
        self.revealed_files.borrow_mut().push_back(path);
    }
    fn link(&self, item: &str) -> Option<Credentials> {
        self.links.borrow_mut().push(format!("link {item}"));
        Some(Credentials {
            socket: "/run/ultra-9000/omp.sock".into(),
            extension: "/run/ultra-9000/ultra9000-status-7.ts".into(),
            token_file: format!("/run/ultra-9000/omp-token-{item}"),
            token: format!("token-{item}"),
        })
    }
    fn unlink(&self, item: &str) {
        self.links.borrow_mut().push(format!("unlink {item}"));
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
    fn notify(&self, item: &str, summary: &str, body: &str) {
        self.notifications
            .borrow_mut()
            .push((item.into(), summary.into(), body.into()));
    }
    fn keep_awake(&self, reason: Option<&'static str>) {
        self.keep_awake.borrow_mut().push(reason);
    }
    fn probe_pinned(&self, pinned: Vec<PathBuf>) {
        self.pin_probes.borrow_mut().push(pinned);
    }
}
