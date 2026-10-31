use super::*;
#[test]
fn lifecycle_state_survives_discrete_event_overflow_in_order() {
    let state = TerminalState {
        title: "done".into(),
        shell: "/bin/zsh".into(),
        pid: Some(1),
        running: false,
        exit_code: Some(0),
        rows: 24,
        cols: 80,
        focused: false,
    };
    let mut queue = HostQueue::default();
    queue.push(1, HostUpdate::State(state.clone()));
    for _ in 0..1024 {
        queue.push(2, HostUpdate::Bell);
    }
    queue.push(3, HostUpdate::State(state));
    assert_eq!(queue.discrete, 512);
    assert_eq!(queue.events.len(), 514);
    assert!(matches!(queue.events.front(), Some((1, HostUpdate::State(s))) if !s.running));
    assert!(matches!(
        queue.events.back(),
        Some((3, HostUpdate::State(_)))
    ));
}
#[test]
fn file_clicks_are_lossless_and_ordered_during_bell_overflow() {
    let mut queue = HostQueue::default();
    queue.push(1, HostUpdate::OpenFile("file:///first".into()));
    for _ in 0..1024 {
        queue.push(2, HostUpdate::Bell);
    }
    queue.push(3, HostUpdate::OpenFile("file:///second".into()));
    assert_eq!(queue.discrete, 512);
    assert_eq!(queue.events.len(), 514);
    assert!(matches!(
        queue.events.front(),
        Some((1, HostUpdate::OpenFile(uri))) if uri == "file:///first"
    ));
    assert!(matches!(
        queue.events.back(),
        Some((3, HostUpdate::OpenFile(uri))) if uri == "file:///second"
    ));
}
#[test]
fn starting_selection_keeps_original_mru_after_background_completion() {
    let a = item("a");
    let mut b = item("b");
    b.process = Process::Starting;
    let mut recent = strings(&["a", "b"]);
    retain_recent(&mut recent, [&a, &b].into_iter());
    assert_eq!(recent, strings(&["a", "b"]));
    assert_eq!(
        recent_running(&recent, [&a, &b].into_iter()),
        strings(&["a"])
    );
    b.process = Process::Running;
    retain_recent(&mut recent, [&a, &b].into_iter());
    assert_eq!(
        recent_running(&recent, [&a, &b].into_iter()),
        strings(&["a", "b"])
    );
    assert_eq!(
        selection_fallback(&recent, &strings(&["b"])),
        Some("b".into())
    );
}
#[test]
fn unseen_fallback_keeps_unread_until_visible_presentation() {
    let mut b = item("b");
    b.unread = true;
    assert_eq!(
        selection_fallback(&strings(&["a", "b"]), &strings(&["b"])),
        Some("b".into())
    );
    assert!(!acknowledge_unread(&mut b, false));
    assert!(b.unread);
    assert!(acknowledge_unread(&mut b, true));
    assert!(!b.unread);
}
#[test]
fn saved_running_binding_tracks_start_exit_and_unbind() {
    let mut i = item("a");
    i.conversation = Some(ConversationRef {
        tool: "omp".into(),
        id: "saved".into(),
        path: "/saved".into(),
    });
    i.process = Process::Starting;
    assert!(running_binding(&i).is_some());
    i.process = Process::Running;
    assert!(running_binding(&i).is_some());
    i.process = Process::Exited;
    assert!(running_binding(&i).is_none());
    i.process = Process::Running;
    i.conversation = None;
    assert!(running_binding(&i).is_none());
}
#[test]
fn symlinked_store_resume_matches_discovery_running_item_id() {
    use serde_json::json;
    use std::fs;
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cwd = dir.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let tools = crate::config::builtins();
    for (tool_id, relative, header) in [
        (
            "omp",
            ".omp/agent/sessions",
            json!({"type":"session","id":"saved","cwd":cwd,"title":"Saved"}),
        ),
        (
            "claude",
            ".claude/projects",
            json!({"type":"assistant","sessionId":"saved","cwd":cwd}),
        ),
        (
            "codex",
            ".codex/sessions",
            json!({"type":"session_meta","payload":{"id":"saved","cwd":cwd,"source":"cli"}}),
        ),
        (
            "pi",
            ".pi/agent/sessions",
            json!({"type":"session","id":"saved","cwd":cwd}),
        ),
    ] {
        let target = dir.path().join(tool_id);
        fs::create_dir_all(target.join("bucket")).unwrap();
        let store = home.join(relative);
        fs::create_dir_all(store.parent().unwrap()).unwrap();
        symlink(&target, &store).unwrap();
        let path = store.join("bucket/date_saved.jsonl");
        fs::write(&path, format!("{header}\n")).unwrap();

        let tool = tools.iter().find(|tool| tool.id == tool_id).unwrap();
        let resumed = crate::discovery::read_bound(tool, "saved", &path, &home, &[]).unwrap();
        assert_eq!(resumed.path, path);
        assert_ne!(resumed.path, path.canonicalize().unwrap());
        let mut index = crate::discovery::Index::new(home.clone());
        index.refresh(&tools, &[]);
        let discovered = index
            .conversations()
            .iter()
            .find(|c| c.tool == tool_id)
            .unwrap()
            .clone();
        assert_eq!(resumed.path, discovered.path);
        if tool_id == "omp" {
            assert_eq!(index.omp_by_session_id("saved").unwrap().path, resumed.path);
        }
        let mut live = item("resumed");
        live.tool = tool_id.into();
        live.conversation = Some(ConversationRef {
            tool: resumed.tool,
            id: resumed.id,
            path: resumed.path.to_string_lossy().into_owned(),
        });
        for process in [Process::Starting, Process::Running] {
            live.process = process;
            let running = find_bound_item(
                [&live].into_iter(),
                &discovered.tool,
                &discovered.id,
                &discovered.path.to_string_lossy(),
            );
            assert_eq!(running.as_deref(), Some("resumed"));
            let wire = serde_json::to_value(discovered.entry(running, false)).unwrap();
            assert_eq!(wire["runningItemId"], "resumed");
        }
    }
}
#[test]
fn new_agent_uses_project_root_but_terminals_use_recorded_directory() {
    let mut i = item("a");
    i.cwd = "/atlas/subdirectory".into();
    assert_eq!(shortcut_cwd(Action::NewAgent, Some(&i), "/home"), "/atlas");
    for action in [Action::NewTerminal, Action::NewNeovim] {
        assert_eq!(
            shortcut_cwd(action, Some(&i), "/home"),
            "/atlas/subdirectory"
        );
    }
    assert_eq!(shortcut_cwd(Action::NewAgent, None, "/home"), "/home");
}
#[test]
fn empty_project_query_filters_before_two_hundred_row_limit() {
    let mut conversations: Vec<_> = (0..201)
        .map(|n| Conversation {
            tool: "omp".into(),
            id: n.to_string(),
            path: PathBuf::from(format!("/saved/{n}")),
            title: "Saved".into(),
            cwd: "/foreign".into(),
            cwd_missing: false,
            project_root: "/foreign".into(),
            created_at_ms: 0,
            updated_at_ms: 1,
            model: None,
        })
        .collect();
    let mut old = conversations[0].clone();
    old.id = "old".into();
    old.project_root = "/atlas".into();
    conversations.push(old);
    let settings = Settings::default();
    assert_eq!(
        matching_conversations(&conversations, Some("/atlas"), "", &settings)
            .take(200)
            .next()
            .unwrap()
            .id,
        "old"
    );
    assert_eq!(
        matching_conversations(&conversations, None, "", &settings).count(),
        202
    );
}
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).into()).collect()
}
fn item(id: &str) -> ItemState {
    ItemState {
        id: id.into(),
        tool: "omp".into(),
        kind: ToolKind::Agent,
        project_id: "/atlas".into(),
        cwd: "/atlas".into(),
        title: "OMP".into(),
        process: Process::Running,
        exit_code: None,
        failure: None,
        activity: Activity::Unknown,
        attention: None,
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
fn foreground_job_needs_two_polls_and_clears_at_once() {
    let mut candidate = None;
    let cargo = || Some((42, "cargo".to_string()));
    // A quick `ls` seen on one poll only never shows.
    assert_eq!(
        next_job(&None, &mut candidate, Some((7, "ls".into())), 1_000),
        None
    );
    assert_eq!(next_job(&None, &mut candidate, None, 2_000), None);
    // The same group on two polls in a row shows, aged from its first sighting.
    assert_eq!(next_job(&None, &mut candidate, cargo(), 3_000), None);
    let shown = next_job(&None, &mut candidate, cargo(), 4_000);
    assert_eq!(
        shown,
        Some(ForegroundJobState {
            command: "cargo".into(),
            since_ms: 3_000
        })
    );
    // A new group keeps the old job until it settles, then replaces it.
    assert_eq!(
        next_job(&shown, &mut candidate, Some((43, "rustc".into())), 5_000),
        shown
    );
    assert_eq!(
        next_job(&shown, &mut candidate, Some((43, "rustc".into())), 6_000)
            .unwrap()
            .command,
        "rustc"
    );
    // Back at the prompt clears on the first idle poll.
    assert_eq!(next_job(&shown, &mut candidate, None, 7_000), None);
}
#[test]
fn close_confirmation_decision_table() {
    let mut settings = Settings::default();
    let mut i = item("a");
    for (activity, expected) in [
        (Activity::Unknown, None),
        (Activity::YourTurn, None),
        (Activity::Error, None),
        (Activity::Working, Some(CloseReason::AgentWorking)),
        (
            Activity::NeedsYou(Attention::Approval),
            Some(CloseReason::AgentNeedsYou),
        ),
        (
            Activity::NeedsYou(Attention::Question),
            Some(CloseReason::AgentNeedsYou),
        ),
    ] {
        i.activity = activity;
        assert_eq!(
            close_reason(&i, StatusSource::Omp, &settings, false),
            expected
        );
    }
    i.tool = "claude".into();
    i.activity = Activity::Unknown;
    assert_eq!(
        close_reason(&i, StatusSource::Process, &settings, false),
        None
    );
    settings.confirm_close_unmonitored_agents = true;
    assert_eq!(
        close_reason(&i, StatusSource::Process, &settings, false),
        Some(CloseReason::UnmonitoredAgent)
    );
    i.kind = ToolKind::Terminal;
    i.tool = "shell".into();
    assert_eq!(
        close_reason(&i, StatusSource::Process, &settings, false),
        None
    );
    assert_eq!(
        close_reason(&i, StatusSource::Process, &settings, true),
        Some(CloseReason::ForegroundJob)
    );
    settings.confirm_close_with_foreground_job = false;
    assert_eq!(
        close_reason(&i, StatusSource::Process, &settings, true),
        None
    );
    i.tool = "neovim".into();
    assert_eq!(
        close_reason(&i, StatusSource::Process, &settings, false),
        Some(CloseReason::Neovim)
    );
    for process in [Process::Starting, Process::Exited, Process::Failed] {
        i.process = process;
        assert_eq!(
            close_reason(&i, StatusSource::Process, &settings, true),
            None
        );
    }
}
#[test]
fn removal_uses_mru_not_adjacent_launch_order() {
    assert_eq!(
        selection_fallback(
            &strings(&["gone", "c", "a", "b"]),
            &strings(&["a", "b", "c"])
        ),
        Some("c".into())
    );
    assert_eq!(selection_fallback(&strings(&["gone"]), &[]), None);
}
#[test]
fn title_derivation() {
    let omp =
        |name, raw, conversation| derived_title(StatusSource::Omp, name, raw, conversation, "OMP");
    assert_eq!(
        omp(Some("Fix login"), "π > atlas", Some("Saved")),
        "Fix login"
    );
    // OMP's own terminal title is never parsed.
    assert_eq!(
        omp(None, "π > atlas", Some("API investigation")),
        "API investigation"
    );
    assert_eq!(omp(Some("  "), "π ! Approval", None), "OMP");
    assert_eq!(
        derived_title(
            StatusSource::Process,
            None,
            "",
            Some("saved"),
            "Claude Code"
        ),
        "saved"
    );
    assert_eq!(
        derived_title(StatusSource::Process, None, "  ", None, "Shell"),
        "Shell"
    );
    assert_eq!(
        derived_title(
            StatusSource::Process,
            Some("ignored"),
            " vim notes ",
            None,
            "Shell"
        ),
        "vim notes"
    );
}
#[test]
fn pinned_then_first_running_project_order() {
    assert_eq!(
        project_order(&strings(&["p2", "p1", "p2"]), &strings(&["p3", "p1", "p4"])),
        strings(&["p2", "p1", "p3", "p4"])
    );
}
#[test]
fn item_numbers_skip_folded_projects() {
    let projects = [
        ("a", &["1", "2"][..]),
        ("b", &["3"][..]),
        ("c", &["4", "5"][..]),
    ]
    .into_iter()
    .map(|(id, items)| ProjectState {
        id: id.into(),
        path: id.into(),
        name: id.into(),
        pinned: false,
        missing: false,
        default_tool: "omp".into(),
        item_ids: strings(items),
    })
    .collect::<Vec<_>>();
    assert_eq!(
        visible_order(&projects, &strings(&["b"])),
        strings(&["1", "2", "4", "5"])
    );
    assert_eq!(
        visible_order(&projects, &strings(&["a", "b", "c"])),
        Vec::<String>::new()
    );
}
#[test]
fn jump_waiting_prioritizes_attention_then_oldest_unread() {
    let mut a = item("new-approval");
    a.activity = Activity::NeedsYou(Attention::Approval);
    a.state_since = 10;
    let mut b = item("old-turn");
    b.activity = Activity::YourTurn;
    b.unread = true;
    b.state_since = 1;
    let mut c = item("old-question");
    c.activity = Activity::NeedsYou(Attention::Question);
    c.state_since = 2;
    let mut d = item("seen-turn");
    d.activity = Activity::YourTurn;
    let order = waiting_cycle_order([a, b, c, d].iter(), &[]);
    assert_eq!(
        order,
        strings(&["old-question", "new-approval", "old-turn"])
    );
    assert_eq!(
        cycle(&order, Some("old-turn"), true),
        Some("old-question".into())
    );
}
#[test]
fn waiting_cycle_drops_working_and_prioritizes_new_attention() {
    let mut seen = item("seen");
    seen.activity = Activity::YourTurn;
    let mut working = item("working");
    working.activity = Activity::Working;
    let mut approval = item("approval");
    approval.activity = Activity::NeedsYou(Attention::Approval);
    approval.state_since = 10;
    let order = waiting_cycle_order(
        [&seen, &working, &approval].into_iter(),
        &strings(&["seen", "working"]),
    );
    assert_eq!(order, strings(&["approval", "seen"]));
}
#[test]
fn wire_shapes_use_item_ids_and_tagged_sheets() {
    let sheet = Sheet::ConfirmClose {
        item_id: "item-1".into(),
        reason: CloseReason::AgentNeedsYou,
        detail: None,
    };
    assert_eq!(
        serde_json::to_value(sheet).unwrap(),
        serde_json::json!({"kind":"confirm-close","itemId":"item-1","reason":"agent-needs-you","detail":null})
    );
    let i = serde_json::to_value(item("item-1")).unwrap();
    assert!(i.get("projectId").is_some());
    assert_eq!(i["process"], "running");
    assert_eq!(i["activity"], "unknown");
}
/// Workspace flows end to end on a headless fake host.
mod flows {
    use super::strings;
    use crate::workspace::{test_support::*, *};
    use serde_json::json;

    fn launch(tool: &str, project: &str) -> String {
        begin_launch(
            tool.into(),
            project.into(),
            project.into(),
            None,
            None,
            None,
            None,
        )
        .unwrap()
    }
    fn running(host: &FakeHost, tool: &str, project: &str) -> (String, SessionKey) {
        let id = launch(tool, project);
        let key = host.complete(&id);
        (id, key)
    }
    /// Status-link lines of OMP's turn states.
    const WORKING: &str = r#"{"type":"state","state":"working"}"#;
    const DONE: &str = r#"{"type":"state","state":"done"}"#;
    const APPROVAL: &str = r#"{"type":"state","state":"needs-you","kind":"approval"}"#;
    const QUESTION: &str = r#"{"type":"state","state":"needs-you","kind":"question"}"#;
    /// The `waiting` line for background `bash` jobs (label, start in epoch ms).
    fn waiting(jobs: &[(&str, u64)]) -> String {
        let jobs: Vec<_> = jobs
            .iter()
            .map(|(label, at)| json!({"label": label, "jobType": "bash", "startedAt": at}))
            .collect();
        json!({"type": "state", "state": "waiting", "jobs": jobs}).to_string()
    }
    fn resume(c: &Conversation) -> Result<String, String> {
        begin_launch(
            c.tool.clone(),
            c.project_root.to_string_lossy().into(),
            c.project_root.to_string_lossy().into(),
            Some(c.clone()),
            None,
            None,
            None,
        )
    }
    fn saved(tool: &str, id: &str, root: &str) -> Conversation {
        Conversation {
            tool: tool.into(),
            id: id.into(),
            path: PathBuf::from(format!("/store/{tool}/{id}.jsonl")),
            title: format!("Saved {id}"),
            cwd: root.into(),
            cwd_missing: false,
            project_root: root.into(),
            created_at_ms: 0,
            updated_at_ms: 0,
            model: Some("opus".into()),
        }
    }
    fn snap() -> WorkspaceState {
        snapshot().unwrap()
    }
    fn item(id: &str) -> ItemState {
        snap()
            .items
            .into_iter()
            .find(|i| i.id == id)
            .unwrap_or_else(|| panic!("{id} is not a row"))
    }
    fn ids() -> Vec<String> {
        snap().items.into_iter().map(|i| i.id).collect()
    }
    fn active() -> Option<String> {
        snap().active_item_id
    }
    fn set(f: impl FnOnce(&mut Workspace)) {
        with(|w| {
            f(w);
            Ok(())
        })
        .unwrap();
    }
    fn sheet() -> serde_json::Value {
        serde_json::to_value(snap().sheet).unwrap()
    }
    /// Revision of the latest snapshot; it advances exactly when one is published.
    fn revision() -> u64 {
        with(|w| Ok(w.revision)).unwrap()
    }
    fn rect(width: f64, height: f64) -> ViewportRect {
        ViewportRect {
            x: 0.,
            y: 0.,
            width,
            height,
            top_left_radius: 0.,
            bottom_left_radius: 0.,
        }
    }
    fn ok_launch() -> Result<Launch, String> {
        Ok(Launch {
            command: None,
            executable: None,
            working_directory: "/".into(),
            env: vec![],
        })
    }
    fn events(host: &FakeHost, name: &str) -> Vec<serde_json::Value> {
        host.events
            .borrow()
            .iter()
            .filter(|(event, _)| event == name)
            .map(|(_, payload)| payload.clone())
            .collect()
    }

    #[test]
    fn uninitialized_and_reentrant_access_fail_without_panicking() {
        let fresh = std::thread::spawn(|| snapshot().map(|_| ()))
            .join()
            .unwrap();
        assert_eq!(fresh.unwrap_err(), "workspace is not initialized");
        install();
        assert_eq!(
            with(|_| snapshot()).map(|_| ()).unwrap_err(),
            "workspace is busy"
        );
    }

    #[test]
    fn every_change_publishes_one_snapshot_with_the_next_revision() {
        let host = install();
        let start = snap().revision;
        let a = launch("omp", "/p/atlas");
        let key = host.complete(&a);
        host.omp(&a, r#"{"type":"name","name":"Fix login"}"#);
        let revisions: Vec<_> = host.published.borrow().iter().map(|s| s.revision).collect();
        assert_eq!(revisions, (start + 1..=start + 3).collect::<Vec<_>>());
        assert_eq!(snap().revision, start + 3);
        // Reads and no-op updates publish nothing.
        snapshot().unwrap();
        notice_dismiss("notice-404").unwrap();
        host.omp(&a, r#"{"type":"name","name":"Fix login"}"#);
        // OMP's terminal title (spinner frames) never touches the item.
        host.retitle(key, "π ⠹ Fix login");
        deliver(key, embed::HostEvent::Bell);
        assert_eq!(host.published.borrow().len(), 3);
        assert_eq!(host.last_published().items[0].title, "Fix login");
    }

    #[test]
    fn launch_publishes_a_starting_row_grouped_by_project_and_selects_it() {
        let host = install();
        let a = launch("omp", "/p/atlas");
        assert_eq!(a, "item-1");
        let s = snap();
        let row = &s.items[0];
        assert_eq!(
            (row.process, row.kind, row.title.as_str(), row.cwd.as_str()),
            (Process::Starting, ToolKind::Agent, "OMP", "/p/atlas")
        );
        assert_eq!(s.active_item_id.as_deref(), Some("item-1"));
        assert!(
            s.recent_item_ids.is_empty(),
            "a starting row is no fallback target yet"
        );
        assert_eq!(
            *host.prepared.borrow(),
            vec![Prepared {
                id: a.clone(),
                tool: "omp".into(),
                cwd: "/p/atlas".into(),
                args: vec![],
                linked: true,
            }]
        );
        assert_eq!(
            begin_launch(
                "nope".into(),
                "/p".into(),
                "/p".into(),
                None,
                None,
                None,
                None
            )
            .unwrap_err(),
            "Unknown tool: nope"
        );
        let b = launch("claude", "/p/zeta");
        let c = launch("shell", "/p/atlas");
        assert_eq!((b.as_str(), c.as_str()), ("item-2", "item-3"));
        let projects: Vec<_> = snap()
            .projects
            .into_iter()
            .map(|p| (p.name, p.pinned, p.item_ids))
            .collect();
        assert_eq!(
            projects,
            vec![
                ("atlas".into(), false, vec![a, c.clone()]),
                ("zeta".into(), false, vec![b])
            ]
        );
        assert_eq!(active(), Some(c));
    }

    #[test]
    fn resume_substitutes_tool_templates_and_extra_args_replace_them() {
        let host = install();
        let omp = saved("omp", "s1", "/p/atlas");
        let id = resume(&omp).unwrap();
        let claude = saved("claude", "s2", "/p/atlas");
        resume(&claude).unwrap();
        begin_launch(
            "claude".into(),
            "/p/atlas".into(),
            "/p/atlas".into(),
            Some(saved("claude", "s3", "/p/atlas")),
            None,
            None,
            Some(vec!["--continue-here".into()]),
        )
        .unwrap();
        begin_launch(
            "neovim".into(),
            "/p/atlas".into(),
            "/p/atlas".into(),
            None,
            None,
            None,
            Some(vec!["notes.md".into()]),
        )
        .unwrap();
        let args: Vec<_> = host
            .prepared
            .borrow()
            .iter()
            .map(|p| p.args.clone())
            .collect();
        assert_eq!(
            args,
            vec![
                strings(&["--resume", "/store/omp/s1.jsonl"]),
                strings(&["--resume", "s2"]),
                strings(&["--continue-here"]),
                strings(&["notes.md"]),
            ]
        );
        let row = item(&id);
        assert_eq!(row.title, "Saved s1");
        assert_eq!(row.model.as_deref(), Some("opus"));
        assert_eq!(
            row.conversation,
            Some(ConversationRef {
                tool: "omp".into(),
                id: "s1".into(),
                path: "/store/omp/s1.jsonl".into(),
            })
        );
        assert_eq!(
            resume(&saved("shell", "s4", "/p")).unwrap_err(),
            "This tool cannot resume conversations."
        );
        // Bound rows changed discovery's running-item view.
        assert!(!events(&host, "conversations-changed").is_empty());
    }

    #[test]
    fn terminals_open_after_their_anchor_in_the_same_project_only() {
        install();
        let a = launch("omp", "/p/one");
        let b = launch("omp", "/p/one");
        let anchored = |tool: &str, project: &str| {
            begin_launch(
                tool.into(),
                format!("{project}/src"),
                project.into(),
                None,
                Some(a.clone()),
                None,
                None,
            )
            .unwrap()
        };
        let t = anchored("shell", "/p/one");
        let other = anchored("shell", "/p/two");
        let agent = anchored("claude", "/p/one");
        assert_eq!(ids(), vec![a.clone(), t.clone(), b, other, agent]);
        assert_eq!(item(&t).cwd, "/p/one/src");
    }

    const EMPTY_ZIP: &[u8] =
        b"PK\x05\x06\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";

    /// An OMP cwd inside its project, linking to a file outside that project.
    fn file_link_source(
        host: &FakeHost,
        contents: &[u8],
    ) -> (tempfile::TempDir, String, SessionKey, String) {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let cwd = project.join("subdirectory");
        std::fs::create_dir_all(&cwd).unwrap();
        let file = dir.path().join("+file;$(touch injected)%#?{cwd}.png");
        std::fs::write(&file, contents).unwrap();
        let uri = tauri::Url::from_file_path(&file).unwrap().to_string();
        let id = begin_launch(
            "omp".into(),
            cwd.to_str().unwrap().into(),
            project.to_str().unwrap().into(),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let key = host.complete(&id);
        (dir, id, key, uri)
    }

    const JAR_NAME: &str = "+launcher;$(touch injected)%#?{cwd}.JAR";

    fn jar_uri(dir: &Path) -> String {
        let path = dir.join(JAR_NAME);
        std::fs::write(&path, EMPTY_ZIP).unwrap();
        tauri::Url::from_file_path(path).unwrap().to_string()
    }

    #[test]
    fn file_click_opens_new_selected_neovim_after_origin_with_exact_cwd_project_and_argv() {
        let host = install();
        let (dir, source, key, uri) = file_link_source(&host, b"text\n");
        host.omp(&source, WORKING);
        let other = launch("omp", "/other-project");
        host_event(key, embed::HostEvent::OpenFile(&uri));
        select(&other).unwrap();
        flush_host_events();
        assert_eq!(host.pending_files(), 1);
        let third = launch("shell", "/third-project");
        assert_eq!(active(), Some(third.clone()));
        host.complete_file();

        let editor = active().unwrap();
        assert_eq!(ids(), vec![source.clone(), editor.clone(), other, third]);
        let row = item(&editor);
        assert_eq!(row.tool, "neovim");
        assert_eq!(row.cwd, item(&source).cwd);
        assert_eq!(row.project_id, item(&source).project_id);
        assert_ne!(row.cwd, dir.path().to_str().unwrap());
        let expected = vec![
            "--".to_owned(),
            dir.path()
                .join("+file;$(touch injected)%#?{cwd}.png")
                .to_str()
                .unwrap()
                .to_owned(),
        ];
        let prepared = host.prepared.borrow().last().unwrap().clone();
        assert_eq!(prepared.tool, "neovim");
        assert_eq!(prepared.cwd, item(&source).cwd);
        assert_eq!(prepared.args, expected);
        host.complete(&editor);
        assert_eq!(active(), Some(editor.clone()));
        assert_eq!(item(&source).process, Process::Running);
        assert_eq!(item(&source).activity, Activity::Working);
        assert_eq!(host.key(&source), Some(key));
        assert!(host.calls_of("stop").is_empty());
        assert!(snap().notices.is_empty());

        // Even an already-open file starts a new item, not a duplicate process in the agent.
        deliver(key, embed::HostEvent::OpenFile(&uri));
        host.complete_file();
        let second = active().unwrap();
        assert_ne!(second, editor);
        assert_eq!(ids()[..3], [source, second, editor]);
    }

    #[test]
    fn external_file_routes_leave_workspace_and_source_unchanged() {
        for reveal in [false, true] {
            for covered in [false, true] {
                let host = install();
                let (dir, source, key, uri) = file_link_source(&host, EMPTY_ZIP);
                let uri = if reveal { jar_uri(dir.path()) } else { uri };
                host.omp(&source, WORKING);
                deliver(key, embed::HostEvent::OpenFile(&uri));
                let other = launch("shell", "/other-project");
                if covered {
                    sheet_open(OpenableSheet::Settings).unwrap();
                }
                let before = serde_json::to_value(snap()).unwrap();
                let prepared = host.prepared.borrow().len();
                host.complete_file();
                assert_eq!(serde_json::to_value(snap()).unwrap(), before);
                assert_eq!(ids(), vec![source.clone(), other.clone()]);
                assert_eq!(active(), Some(other));
                assert_eq!(host.prepared.borrow().len(), prepared);
                let expected = dir.path().join(if reveal {
                    JAR_NAME
                } else {
                    "+file;$(touch injected)%#?{cwd}.png"
                });
                if reveal {
                    assert!(host.system_files.borrow().is_empty());
                    assert_eq!(
                        host.revealed_files.borrow().front().map(String::as_str),
                        expected.to_str()
                    );
                    host.complete_reveal_file(Ok(()));
                } else {
                    assert!(host.revealed_files.borrow().is_empty());
                    assert_eq!(
                        host.system_files.borrow().front().map(String::as_str),
                        expected.to_str()
                    );
                    host.complete_system_file(Ok(()));
                }
                assert_eq!(item(&source).process, Process::Running);
                assert_eq!(item(&source).activity, Activity::Working);
                assert_eq!(host.key(&source), Some(key));
                assert!(host.calls_of("stop").is_empty());
                assert_eq!(serde_json::to_value(snap()).unwrap(), before);
            }
        }
    }

    #[test]
    fn external_failure_is_visible_without_creating_an_editor_or_closing_the_sheet() {
        for reveal in [false, true] {
            let host = install();
            let (dir, source, key, uri) = file_link_source(&host, EMPTY_ZIP);
            let uri = if reveal { jar_uri(dir.path()) } else { uri };
            deliver(key, embed::HostEvent::OpenFile(&uri));
            sheet_open(OpenableSheet::Settings).unwrap();
            let before_sheet = sheet();
            host.complete_file();
            assert!(snap().notices.is_empty());
            let result = Err("desktop dispatch failed".into());
            if reveal {
                assert!(host.system_files.borrow().is_empty());
                host.complete_reveal_file(result);
            } else {
                assert!(host.revealed_files.borrow().is_empty());
                host.complete_system_file(result);
            }
            assert_eq!(ids(), vec![source.clone()]);
            assert_eq!(active(), Some(source.clone()));
            assert_eq!(sheet(), before_sheet);
            assert_eq!(host.prepared.borrow().len(), 1);
            assert_eq!(host.key(&source), Some(key));
            assert_eq!(item(&source).process, Process::Running);
            let notice = snap().notices.pop().unwrap();
            assert_eq!(notice.kind, "error");
            assert_eq!(notice.body, "desktop dispatch failed");
            assert!(host.calls_of("stop").is_empty());
        }
    }

    #[test]
    fn file_dispatch_errors_do_not_fall_back_to_the_other_route() {
        for contents in [b"text\n".as_slice(), EMPTY_ZIP] {
            let host = install();
            let (_dir, source, key, uri) = file_link_source(&host, contents);
            with(|w| {
                w.settings.tools.retain(|tool| tool.id != "neovim");
                Ok(())
            })
            .unwrap();
            deliver(key, embed::HostEvent::OpenFile(&uri));
            host.complete_file();
            assert_eq!(ids(), vec![source.clone()]);
            assert_eq!(active(), Some(source.clone()));
            assert_eq!(host.prepared.borrow().len(), 1);
            if contents == EMPTY_ZIP {
                assert_eq!(host.system_files.borrow().len(), 1);
                assert!(snap().notices.is_empty());
                host.complete_system_file(Ok(()));
            } else {
                assert!(host.system_files.borrow().is_empty());
                let notice = snap().notices.pop().unwrap();
                assert_eq!(notice.kind, "error");
                assert!(!notice.body.is_empty());
            }
            assert_eq!(item(&source).process, Process::Running);
            assert!(host.calls_of("stop").is_empty());
        }
    }

    #[test]
    fn invalid_file_clicks_show_errors_without_preparing_or_stopping_a_process() {
        let host = install();
        let (dir, source, key, _) = file_link_source(&host, b"text\n");
        let missing = tauri::Url::from_file_path(dir.path().join("missing"))
            .unwrap()
            .to_string();
        let directory = tauri::Url::from_file_path(dir.path()).unwrap().to_string();
        for uri in [
            missing.as_str(),
            directory.as_str(),
            "file://remote.example/etc/passwd",
            "file:///bad%ZZ",
            "file:///dev/null",
        ] {
            deliver(key, embed::HostEvent::OpenFile(uri));
            host.complete_file();
            assert_eq!(ids(), vec![source.clone()]);
            assert_eq!(host.prepared.borrow().len(), 1);
            assert_eq!(active(), Some(source.clone()));
            let notice = snap().notices.pop().unwrap();
            assert_eq!(notice.kind, "error");
            assert!(!notice.body.is_empty());
        }
        assert!(host.system_files.borrow().is_empty());
        assert!(host.calls_of("stop").is_empty());
    }

    #[test]
    fn stale_file_classification_results_cannot_launch_open_or_report_errors() {
        for route in ["text", "system", "reveal", "invalid"] {
            for race in ["closing", "removed", "quitting", "restarted"] {
                let host = install();
                let (dir, source, key, uri) = file_link_source(
                    &host,
                    if route == "text" {
                        b"text\n"
                    } else {
                        EMPTY_ZIP
                    },
                );
                let uri = match route {
                    "invalid" => "file://remote.example/etc/passwd".into(),
                    "reveal" => jar_uri(dir.path()),
                    _ => uri,
                };
                deliver(key, embed::HostEvent::OpenFile(&uri));
                assert_eq!(host.pending_files(), 1);
                match race {
                    "closing" => {
                        close(&source, true).unwrap();
                    }
                    "removed" => {
                        close(&source, true).unwrap();
                        host.exit(key, Some(0));
                    }
                    "quitting" => {
                        quit_snapshot().unwrap();
                    }
                    "restarted" => {
                        host.exit(key, Some(7));
                        let (tool, cwd, project, conversation, extra) =
                            restart_target(&source).unwrap();
                        let restarted = begin_launch(
                            tool,
                            cwd,
                            project,
                            conversation,
                            None,
                            Some(source),
                            extra,
                        )
                        .unwrap();
                        host.complete(&restarted);
                    }
                    _ => unreachable!(),
                }
                let prepared = host.prepared.borrow().len();
                let before = ids();
                let selected = active();
                host.complete_file();
                assert_eq!(ids(), before, "{race}");
                assert_eq!(active(), selected, "{race}");
                assert_eq!(host.prepared.borrow().len(), prepared, "{race}");
                assert!(host.system_files.borrow().is_empty(), "{race}");
                assert!(host.revealed_files.borrow().is_empty(), "{race}");
                assert!(
                    snap().notices.iter().all(|notice| notice.kind != "error"),
                    "{race}"
                );
            }
        }
    }

    #[test]
    fn stale_queued_file_events_do_not_start_validation() {
        for race in ["closing", "removed", "quitting", "unknown-key"] {
            let host = install();
            let (_dir, source, key, uri) = file_link_source(&host, b"text\n");
            match race {
                "closing" => {
                    close(&source, true).unwrap();
                }
                "removed" => {
                    close(&source, true).unwrap();
                    host.exit(key, Some(0));
                }
                "quitting" => {
                    quit_snapshot().unwrap();
                }
                _ => {}
            }
            let event_key = if race == "unknown-key" {
                key + 999
            } else {
                key
            };
            deliver(event_key, embed::HostEvent::OpenFile(&uri));
            assert_eq!(host.pending_files(), 0, "{race}");
            assert_eq!(host.prepared.borrow().len(), 1, "{race}");
            assert!(snap().notices.is_empty(), "{race}");
            assert!(host.system_files.borrow().is_empty(), "{race}");
        }
    }

    #[test]
    fn retained_exited_output_keeps_all_file_routes_usable_even_during_classification() {
        for route in ["text", "system", "reveal"] {
            for exits_before_click in [false, true] {
                let host = install();
                let (dir, source, key, uri) = file_link_source(
                    &host,
                    if route == "text" {
                        b"text\n"
                    } else {
                        EMPTY_ZIP
                    },
                );
                let uri = if route == "reveal" {
                    jar_uri(dir.path())
                } else {
                    uri
                };
                if exits_before_click {
                    host.exit(key, Some(7));
                }
                deliver(key, embed::HostEvent::OpenFile(&uri));
                if !exits_before_click {
                    host.exit(key, Some(7));
                }
                host.complete_file();
                assert_eq!(item(&source).process, Process::Exited);
                if route != "text" {
                    assert_eq!(ids(), vec![source.clone()]);
                    assert_eq!(active(), Some(source));
                    assert_eq!(host.prepared.borrow().len(), 1);
                    if route == "reveal" {
                        assert!(host.system_files.borrow().is_empty());
                        assert_eq!(host.revealed_files.borrow().len(), 1);
                        host.complete_reveal_file(Ok(()));
                    } else {
                        assert!(host.revealed_files.borrow().is_empty());
                        assert_eq!(host.system_files.borrow().len(), 1);
                        host.complete_system_file(Ok(()));
                    }
                } else {
                    let editor = active().unwrap();
                    assert_ne!(editor, source);
                    assert_eq!(item(&editor).tool, "neovim");
                    assert_eq!(item(&editor).cwd, item(&source).cwd);
                    host.complete(&editor);
                    assert_eq!(item(&editor).process, Process::Running);
                    assert!(host.system_files.borrow().is_empty());
                    assert!(host.revealed_files.borrow().is_empty());
                }
                assert!(host.calls_of("stop").is_empty());
            }
        }
    }

    #[test]
    fn accepted_file_open_is_an_independent_sibling_if_origin_closes_during_preparation() {
        let host = install();
        let (_dir, source, key, uri) = file_link_source(&host, b"text\n");
        deliver(key, embed::HostEvent::OpenFile(&uri));
        host.complete_file();
        let editor = active().unwrap();
        assert_eq!(item(&editor).process, Process::Starting);
        close(&source, true).unwrap();
        host.exit(key, Some(0));
        host.complete(&editor);
        assert_eq!(ids(), vec![editor.clone()]);
        assert_eq!(item(&editor).process, Process::Running);
        assert_eq!(active(), Some(editor));
        assert!(snap().notices.is_empty());
    }

    #[test]
    fn file_open_uses_normal_missing_neovim_failure_and_preserves_file_args_on_restart() {
        let host = install();
        let (_dir, source, key, uri) = file_link_source(&host, b"text\n");
        deliver(key, embed::HostEvent::OpenFile(&uri));
        host.complete_file();
        let editor = active().unwrap();
        let prepared = host.prepared.borrow().last().unwrap().clone();
        host.fail(
            &editor,
            "Neovim (nvim) was not found on your login shell's PATH.",
        );
        assert_eq!(item(&editor).process, Process::Failed);
        assert_eq!(
            snap().notices.last().unwrap().body,
            item(&editor).failure.unwrap()
        );
        let (tool, cwd, project, conversation, extra) = restart_target(&editor).unwrap();
        assert_eq!(cwd, item(&source).cwd);
        assert_eq!(project, item(&source).project_id);
        assert_eq!(extra, Some(prepared.args.clone()));
        let restarted =
            begin_launch(tool, cwd, project, conversation, None, Some(editor), extra).unwrap();
        assert_eq!(host.prepared.borrow().last().unwrap().args, prepared.args);
        assert_eq!(ids(), vec![source.clone(), restarted.clone()]);
        host.complete(&restarted);
        assert_eq!(item(&source).process, Process::Running);
        assert_eq!(active(), Some(restarted));
        assert!(host.calls_of("stop").is_empty());
    }

    #[test]
    fn open_elsewhere_selects_the_running_row_instead_of_duplicating_it() {
        let host = install();
        let c = saved("omp", "s1", "/p/atlas");
        let first = resume(&c).unwrap();
        let other = launch("omp", "/p/atlas");
        assert_eq!(active(), Some(other));
        assert_eq!(resume(&c).unwrap(), first);
        assert_eq!(active(), Some(first.clone()));
        assert_eq!(ids().len(), 2);
        assert_eq!(host.prepared.borrow().len(), 2);
        let path = "/store/omp/s1.jsonl";
        assert_eq!(
            select_bound("omp", "s1", path).unwrap(),
            Some(first.clone())
        );
        assert_eq!(select_bound("claude", "s1", path).unwrap(), None);
        // Same id under another tool is a different conversation.
        let mut claude = c.clone();
        claude.tool = "claude".into();
        assert_ne!(resume(&claude).unwrap(), first);
        // A failed row no longer holds the conversation.
        host.fail(&first, "omp: command not found");
        let again = resume(&c).unwrap();
        assert_ne!(again, first);
        assert_eq!(item(&again).process, Process::Starting);
    }

    #[test]
    fn finishing_a_launch_runs_the_session_with_unknown_status_until_omp_reports() {
        let host = install();
        *host.initial_title.borrow_mut() = "π ⠋ Fix login".into();
        let id = launch("omp", "/p/atlas");
        let key = host.complete(&id);
        let row = item(&id);
        assert_eq!(row.process, Process::Running);
        assert_eq!(row.pid, Some(1000 + key as u32));
        // OMP's terminal title is no status or title source.
        assert_eq!(row.activity, Activity::Unknown);
        assert_eq!(row.title, "OMP");
        assert_eq!(snap().recent_item_ids, vec![id.clone()]);
        // A late result for a row that is no longer starting changes nothing.
        let before = host.published.borrow().len();
        finish_launch(&id, Err("late".into()));
        assert_eq!(item(&id).process, Process::Running);
        assert_eq!(host.published.borrow().len(), before);
        // Other tools show their terminal title.
        *host.initial_title.borrow_mut() = "htop".into();
        let shell = launch("shell", "/p");
        host.complete(&shell);
        assert_eq!(item(&shell).title, "htop");
    }

    #[test]
    fn launch_failures_mark_the_row_failed_and_raise_a_notice() {
        let host = install();
        set(|w| w.host_error = Some("The terminal host is still starting.".into()));
        let early = launch("shell", "/p");
        finish_launch(&early, ok_launch());
        set(|w| w.host_error = None);
        let missing = launch("omp", "/p");
        host.fail(&missing, "omp was not found on PATH");
        *host.create_error.borrow_mut() = Some("pty: out of descriptors".into());
        let pty = launch("shell", "/p");
        finish_launch(&pty, ok_launch());
        let failures: Vec<_> = [&early, &missing, &pty]
            .iter()
            .map(|id| {
                let row = item(id);
                assert_eq!(row.process, Process::Failed);
                row.failure.unwrap()
            })
            .collect();
        assert_eq!(
            failures,
            strings(&[
                "The terminal host is still starting.",
                "omp was not found on PATH",
                "pty: out of descriptors"
            ])
        );
        assert!(host.calls_of("create").is_empty());
        // The failed OMP launch's status-link token is invalidated.
        assert_eq!(
            *host.links.borrow(),
            strings(&["link item-2", "unlink item-2"])
        );
        let notices: Vec<_> = snap()
            .notices
            .into_iter()
            .map(|n| (n.kind, n.title, n.body))
            .collect();
        assert_eq!(notices.len(), 3);
        assert!(notices
            .iter()
            .zip(&failures)
            .all(|(n, f)| n.0 == "error" && n.1 == "Action failed" && &n.2 == f));
        assert!(snap().recent_item_ids.is_empty());
        assert_eq!(active(), Some(pty));
    }

    #[test]
    fn a_launch_closed_while_starting_is_dropped_and_ignores_its_result() {
        let host = install();
        let id = launch("omp", "/p/atlas");
        assert_eq!(close(&id, false).unwrap(), "closed");
        host.fail(&id, "too late");
        let s = snap();
        assert!(s.items.is_empty() && s.projects.is_empty() && s.notices.is_empty());
        assert_eq!(s.active_item_id, None);
    }

    #[test]
    fn restart_replaces_an_ended_row_in_place() {
        let host = install();
        let (a, _) = running(&host, "shell", "/p");
        let b = launch("omp", "/p");
        host.fail(&b, "boom");
        let (c, kc) = running(&host, "shell", "/p");
        assert_eq!(
            restart_target(&a).unwrap_err(),
            "Only exited or failed items can restart."
        );
        let (tool, cwd, project, conversation, extra) = restart_target(&b).unwrap();
        assert_eq!(
            (tool.as_str(), cwd.as_str(), project.as_str()),
            ("omp", "/p", "/p")
        );
        assert!(conversation.is_none() && extra.is_none());
        let d = begin_launch(
            tool,
            cwd,
            project,
            conversation,
            None,
            Some(b.clone()),
            extra,
        )
        .unwrap();
        assert_eq!(ids(), vec![a.clone(), d.clone(), c.clone()]);
        assert_eq!(active(), Some(d));
        assert_eq!(
            begin_launch(
                "shell".into(),
                "/p".into(),
                "/p".into(),
                None,
                None,
                Some(a.clone()),
                None
            )
            .unwrap_err(),
            "Only exited or failed items can restart."
        );
        assert_eq!(ids().len(), 3);
        // An exited session is released before its replacement takes the slot.
        host.exit(kc, Some(2));
        assert_eq!(item(&c).process, Process::Exited);
        let e = begin_launch(
            "shell".into(),
            "/p".into(),
            "/p".into(),
            None,
            None,
            Some(c.clone()),
            None,
        )
        .unwrap();
        assert_eq!(host.calls_of("remove"), vec![kc]);
        assert_eq!(ids().last(), Some(&e));
        assert!(!snap().recent_item_ids.contains(&c));
    }

    #[test]
    fn restart_target_prefers_discovery_and_falls_back_to_the_row() {
        let host = install();
        let conv = saved("claude", "s9", "/p");
        let id = resume(&conv).unwrap();
        let key = host.complete(&id);
        host.exit(key, Some(1));
        let (_, _, _, fallback, _) = restart_target(&id).unwrap();
        let fallback = fallback.unwrap();
        assert_eq!(
            (fallback.id.as_str(), fallback.title.as_str()),
            ("s9", "Saved s9")
        );
        assert_eq!(fallback.path, conv.path);
        assert_eq!(fallback.project_root, PathBuf::from("/p"));
        assert_eq!(fallback.model.as_deref(), Some("opus"));
        let mut renamed = conv.clone();
        renamed.title = "Renamed".into();
        apply_discovery(vec![renamed], vec![]);
        let (_, _, _, found, _) = restart_target(&id).unwrap();
        assert_eq!(found.unwrap().title, "Renamed");
        // Restarting the crashed conversation reuses its slot and session-free row.
        let changes = events(&host, "conversations-changed").len();
        let (tool, cwd, project, conversation, extra) = restart_target(&id).unwrap();
        let again = begin_launch(tool, cwd, project, conversation, None, Some(id), extra).unwrap();
        assert_eq!(ids(), vec![again.clone()]);
        assert_eq!(item(&again).conversation.unwrap().id, "s9");
        assert_eq!(
            host.prepared.borrow().last().unwrap().args,
            strings(&["--resume", "s9"])
        );
        assert_eq!(host.calls_of("remove"), vec![key]);
        assert!(events(&host, "conversations-changed").len() > changes);
    }

    #[test]
    fn restarting_a_row_whose_conversation_is_open_elsewhere_selects_that_row() {
        let host = install();
        let conv = saved("claude", "s1", "/p");
        let old = resume(&conv).unwrap();
        let key = host.complete(&old);
        host.exit(key, Some(1));
        let open = resume(&conv).unwrap();
        assert_ne!(open, old);
        assert_eq!(
            begin_launch(
                "claude".into(),
                "/p".into(),
                "/p".into(),
                Some(conv.clone()),
                None,
                Some(open.clone()),
                None
            )
            .unwrap_err(),
            "Only exited or failed items can restart."
        );
        let selected = begin_launch(
            "claude".into(),
            "/p".into(),
            "/p".into(),
            Some(conv),
            None,
            Some(old),
            None,
        )
        .unwrap();
        assert_eq!(selected, open);
        assert_eq!(ids(), vec![open.clone()]);
        assert_eq!(active(), Some(open));
    }

    #[test]
    fn closing_an_ended_row_removes_it_and_selects_the_most_recent_one() {
        let host = install();
        let (a, _) = running(&host, "shell", "/p/one");
        let (b, kb) = running(&host, "shell", "/p/two");
        let (c, kc) = running(&host, "shell", "/p/one");
        select(&a).unwrap();
        select(&c).unwrap();
        host.exit(kc, Some(3));
        assert_eq!(close(&c, false).unwrap(), "closed");
        assert_eq!(host.calls_of("remove"), vec![kc]);
        assert_eq!(active(), Some(a.clone()), "MRU, not the adjacent row");
        assert_eq!(
            close("item-99", false).unwrap_err(),
            "Item item-99 does not exist."
        );
        // A running plain shell stops at once; the row goes when the process does.
        assert_eq!(close(&b, false).unwrap(), "closed");
        assert_eq!(host.calls_of("stop"), vec![kb]);
        assert!(ids().contains(&b));
        host.exit(kb, None);
        assert_eq!(ids(), vec![a]);
        let names: Vec<_> = snap().projects.into_iter().map(|p| p.name).collect();
        assert_eq!(names, strings(&["one"]));
    }

    #[test]
    fn a_row_that_ends_during_its_close_question_takes_the_question_with_it() {
        let host = install();
        let (n, key) = running(&host, "neovim", "/p");
        assert_eq!(close(&n, false).unwrap(), "confirm");
        host.exit(key, Some(0));
        let s = snap();
        assert!(s.items.is_empty() && s.sheet.is_none());
        assert_eq!((s.view, s.active_item_id), ("terminal", None));
    }

    #[test]
    fn closing_running_neovim_asks_first_then_stops_once() {
        let host = install();
        let (n, key) = running(&host, "neovim", "/p");
        let epoch = snap().presentation_epoch;
        assert_eq!(close(&n, false).unwrap(), "confirm");
        assert_eq!(
            sheet(),
            json!({"kind":"confirm-close","itemId":n,"reason":"neovim","detail":null})
        );
        let s = snap();
        assert_eq!((s.view, s.presentation_epoch), ("sheet", epoch + 1));
        assert_eq!(host.calls_of("hide"), vec![key]);
        assert!(host.calls_of("stop").is_empty());
        assert_eq!(close(&n, true).unwrap(), "closed");
        assert_eq!(host.calls_of("stop"), vec![key]);
        assert_eq!(snap().view, "terminal");
        assert_eq!(close(&n, false).unwrap(), "closed");
        assert_eq!(host.calls_of("stop"), vec![key], "already closing");
        host.exit(key, None);
        assert!(ids().is_empty());
        assert_eq!(host.calls_of("remove"), vec![key]);
        assert!(
            snap().notices.is_empty(),
            "terminals have nothing to resume"
        );
    }

    #[test]
    fn close_shortcut_confirms_the_open_question_for_the_active_row() {
        let host = install();
        dispatch(Action::Close).unwrap();
        assert!(snap().sheet.is_none());
        let (_, key) = running(&host, "neovim", "/p");
        dispatch(Action::Close).unwrap();
        assert_eq!(sheet()["kind"], "confirm-close");
        dispatch(Action::Close).unwrap();
        assert_eq!(host.calls_of("stop"), vec![key]);
        assert!(snap().sheet.is_none());
    }

    #[test]
    fn closing_a_shell_running_a_job_names_it_and_honours_the_setting() {
        let host = install();
        let (s, key) = running(&host, "shell", "/p");
        host.set_job(key, Some((77, "cargo")));
        assert_eq!(close(&s, false).unwrap(), "confirm");
        assert_eq!(sheet()["reason"], "foreground-job");
        assert_eq!(sheet()["detail"], "cargo");
        let shell = snap().tools.into_iter().find(|t| t.id == "shell").unwrap();
        assert_eq!(shell.path.as_deref(), Some("/bin/sh"));
        set(|w| w.settings.confirm_close_with_foreground_job = false);
        sheet_close().unwrap();
        assert_eq!(close(&s, false).unwrap(), "closed");
        assert_eq!(host.calls_of("stop"), vec![key]);
    }

    #[test]
    fn a_clean_agent_exit_removes_the_row_and_announces_the_saved_conversation() {
        let host = install();
        let (other, _) = running(&host, "shell", "/p/atlas");
        let conv = saved("omp", "s1", "/p/atlas");
        let id = resume(&conv).unwrap();
        let key = host.complete(&id);
        let changed = events(&host, "conversations-changed").len();
        host.exit(key, Some(0));
        assert_eq!(ids(), vec![other.clone()]);
        assert_eq!(active(), Some(other));
        assert_eq!(host.calls_of("remove"), vec![key]);
        let notice = serde_json::to_value(&snap().notices).unwrap();
        assert_eq!(notice[0]["kind"], "ended");
        assert_eq!(notice[0]["title"], "“Saved s1” ended");
        assert_eq!(notice[0]["body"], "Conversation saved in atlas.");
        assert_eq!(notice[0]["projectId"], "/p/atlas");
        assert_eq!(notice[0]["conversation"]["id"], "s1");
        assert!(events(&host, "conversations-changed").len() > changed);
    }

    #[test]
    fn an_agent_closed_by_the_user_also_announces_its_conversation() {
        let host = install();
        let id = resume(&saved("omp", "s1", "/p")).unwrap();
        let key = host.complete(&id);
        assert_eq!(close(&id, false).unwrap(), "closed");
        host.exit(key, Some(143));
        assert!(ids().is_empty());
        assert_eq!(snap().notices[0].kind, "ended");
    }

    #[test]
    fn an_agent_error_exit_keeps_the_row_until_closed() {
        let host = install();
        let (id, key) = running(&host, "claude", "/p");
        host.exit(key, Some(1));
        let row = item(&id);
        assert_eq!(
            (row.process, row.exit_code, row.activity),
            (Process::Exited, Some(1), Activity::Error)
        );
        assert_eq!(snap().recent_item_ids, vec![id.clone()]);
        assert!(snap().notices.is_empty());
        assert_eq!(close(&id, false).unwrap(), "closed");
        assert!(ids().is_empty());
        assert_eq!(host.calls_of("remove"), vec![key]);
    }

    #[test]
    fn a_clean_shell_exit_follows_remove_on_clean_exit() {
        let host = install();
        let (_, ka) = running(&host, "shell", "/p");
        host.exit(ka, Some(0));
        assert!(ids().is_empty());
        set(|w| w.settings.remove_on_clean_exit = false);
        let (b, kb) = running(&host, "shell", "/p");
        host.exit(kb, Some(0));
        let row = item(&b);
        assert_eq!(
            (row.process, row.activity),
            (Process::Exited, Activity::Unknown)
        );
    }

    #[test]
    fn working_keep_awake_holds_then_releases_when_the_agent_becomes_idle() {
        let host = install();
        set(|w| w.settings.keep_awake = crate::config::KeepAwake::Working);
        let (id, _) = running(&host, "omp", "/p");
        host.keep_awake.borrow_mut().clear();
        host.omp(&id, WORKING);
        host.omp(&id, r#"{"type":"state","state":"idle"}"#);
        assert_eq!(
            *host.keep_awake.borrow(),
            vec![Some("an agent is working"), None]
        );
    }

    #[test]
    fn unseen_turns_and_questions_mark_unread_and_notify_per_settings() {
        let host = install();
        set(|w| w.settings.notify_turn_finished = true);
        let (id, _) = running(&host, "omp", "/p/atlas");
        host.omp(&id, WORKING);
        assert_eq!(
            (item(&id).activity, item(&id).unread),
            (Activity::Working, false)
        );
        host.omp(&id, DONE);
        let row = item(&id);
        assert_eq!((row.activity, row.unread), (Activity::YourTurn, true));
        host.omp(&id, APPROVAL);
        assert_eq!(item(&id).attention, Some(Attention::Approval));
        set(|w| w.settings.notify_needs_you = false);
        host.omp(&id, QUESTION);
        assert_eq!(item(&id).attention, Some(Attention::Question));
        assert_eq!(
            *host.notifications.borrow(),
            vec![
                (id.clone(), "OMP".to_string(), "Your turn".to_string()),
                (id.clone(), "OMP".to_string(), "Needs you".to_string())
            ]
        );
    }

    #[test]
    fn an_idle_report_is_your_turn_without_unread_or_notification() {
        let host = install();
        set(|w| w.settings.notify_turn_finished = true);
        let (id, _) = running(&host, "omp", "/p/atlas");
        let idle = r#"{"type":"state","state":"idle"}"#;
        // Startup, an aborted turn, and a question answered by aborting are all silent.
        host.omp(&id, idle);
        assert_eq!(
            (item(&id).activity, item(&id).unread),
            (Activity::YourTurn, false)
        );
        host.omp(&id, WORKING);
        host.omp(&id, idle);
        host.omp(&id, QUESTION);
        host.omp(&id, idle);
        let row = item(&id);
        assert_eq!((row.activity, row.unread), (Activity::YourTurn, false));
        assert_eq!(
            *host.notifications.borrow(),
            vec![(id.clone(), "OMP".to_string(), "Needs you".to_string())]
        );
        // A finished turn after idle is announced, even without passing through working.
        host.omp(&id, DONE);
        let row = item(&id);
        assert_eq!((row.activity, row.unread), (Activity::YourTurn, true));
        assert_eq!(
            host.notifications.borrow().last(),
            Some(&(id.clone(), "OMP".to_string(), "Your turn".to_string()))
        );
        assert_eq!(host.notifications.borrow().len(), 2);
    }

    #[test]
    fn finished_turns_stay_silent_by_default_but_still_mark_unread() {
        let host = install();
        let (id, _) = running(&host, "omp", "/p/atlas");
        host.omp(&id, WORKING);
        host.omp(&id, DONE);
        let row = item(&id);
        assert_eq!((row.activity, row.unread), (Activity::YourTurn, true));
        assert!(host.notifications.borrow().is_empty());
        host.omp(&id, APPROVAL);
        assert_eq!(
            *host.notifications.borrow(),
            vec![(id.clone(), "OMP".to_string(), "Needs you".to_string())]
        );
    }

    #[test]
    fn an_agent_on_a_background_job_shows_its_jobs_silently_until_the_turn_settles() {
        let host = install();
        set(|w| w.settings.notify_turn_finished = true);
        host.focused.set(false);
        let (id, _) = running(&host, "omp", "/p/atlas");
        host.omp(&id, WORKING);
        host.omp(
            &id,
            &waiting(&[("cargo build", 200), ("bun run test", 100)]),
        );
        let row = item(&id);
        assert_eq!(
            (row.activity, row.attention, row.unread),
            (Activity::OnJob, None, false)
        );
        assert!(host.notifications.borrow().is_empty(), "entry is silent");
        let wire = serde_json::to_value(&row).unwrap();
        assert_eq!(wire["activity"], "on-job");
        assert_eq!(
            wire["backgroundJobs"],
            json!([
                {"kind": "bash", "label": "bun run test", "startedAtMs": 100},
                {"kind": "bash", "label": "cargo build", "startedAtMs": 200},
            ])
        );
        // Closing asks first, as for a working agent.
        assert_eq!(
            close_reason(&row, StatusSource::Omp, &Settings::default(), false),
            Some(CloseReason::AgentWorking)
        );

        // A job ending changes the list, not how long the agent has been waiting.
        set(|w| w.records[0].state.state_since = 7);
        host.omp(&id, &waiting(&[("cargo build", 200)]));
        let row = item(&id);
        assert_eq!((row.state_since, row.background_jobs.len()), (7, 1));
        assert_eq!(host.last_published().items[0].background_jobs.len(), 1);

        // Waking or a question leaves the state and drops the jobs.
        host.omp(&id, WORKING);
        assert!(item(&id).background_jobs.is_empty());
        host.omp(&id, &waiting(&[("cargo build", 200)]));
        host.omp(&id, APPROVAL);
        let row = item(&id);
        assert_eq!(
            (row.activity, row.background_jobs.len()),
            (Activity::NeedsYou(Attention::Approval), 0)
        );
        // Settling: `idle` is a silent your turn, `done` a finished one.
        host.omp(&id, &waiting(&[("cargo build", 200)]));
        host.notifications.borrow_mut().clear();
        host.omp(&id, r#"{"type":"state","state":"idle"}"#);
        let row = item(&id);
        assert_eq!((row.activity, row.unread), (Activity::YourTurn, false));
        assert!(host.notifications.borrow().is_empty());
        host.omp(&id, &waiting(&[("cargo build", 200)]));
        host.omp(&id, DONE);
        let row = item(&id);
        assert_eq!(
            (row.activity, row.unread, row.background_jobs.len()),
            (Activity::YourTurn, true, 0)
        );
        assert_eq!(
            *host.notifications.borrow(),
            vec![(id.clone(), "OMP".to_string(), "Your turn".to_string())]
        );
    }

    #[test]
    fn every_reported_omp_state_maps_to_the_rows_activity() {
        let host = install();
        let (id, _) = running(&host, "omp", "/p/atlas");
        for (line, activity) in [
            (WORKING, Activity::Working),
            (APPROVAL, Activity::NeedsYou(Attention::Approval)),
            (QUESTION, Activity::NeedsYou(Attention::Question)),
            (
                r#"{"type":"state","state":"waiting","jobs":[{"label":"bun run test","jobType":"bash","startedAt":1791653576243}]}"#,
                Activity::OnJob,
            ),
            (DONE, Activity::YourTurn),
            (r#"{"type":"state","state":"error"}"#, Activity::Error),
            (r#"{"type":"state","state":"idle"}"#, Activity::YourTurn),
        ] {
            host.omp(&id, line);
            let row = item(&id);
            assert_eq!(
                (row.activity, row.attention),
                (activity, activity.attention())
            );
        }
        // Shells and other agents have no status link and stay unknown.
        let (claude, _) = running(&host, "claude", "/p/atlas");
        host.omp(&claude, WORKING);
        assert_eq!(item(&claude).activity, Activity::Unknown);
        assert_eq!(*host.links.borrow(), strings(&["link item-1"]));
        assert!(host.prepared.borrow()[0].linked);
        assert!(!host.prepared.borrow()[1].linked);
    }

    #[test]
    fn a_newer_status_connection_supersedes_the_older_one() {
        let host = install();
        let (id, _) = running(&host, "omp", "/p/atlas");
        host.omp_on(&id, 3, WORKING);
        // A late message from an earlier connection is stale.
        host.omp_on(&id, 2, DONE);
        assert_eq!(item(&id).activity, Activity::Working);
        host.omp_on(&id, 4, APPROVAL);
        assert_eq!(item(&id).activity, Activity::NeedsYou(Attention::Approval));
    }

    #[test]
    fn status_link_tokens_end_with_the_process_or_the_row() {
        let host = install();
        let (a, ka) = running(&host, "omp", "/p");
        host.omp(&a, WORKING);
        assert!(close(&a, true).is_ok());
        host.exit(ka, Some(0));
        assert!(ids().is_empty());
        // An unclean exit keeps the row, but its token ends with the process.
        let (b, kb) = running(&host, "omp", "/p");
        host.exit(kb, Some(1));
        assert_eq!(item(&b).process, Process::Exited);
        // Messages that arrive after the exit change nothing.
        host.omp(&b, WORKING);
        assert_eq!(item(&b).activity, Activity::Error);
        close(&b, false).unwrap();
        // A row closed while starting never launches with a live token.
        let c = launch("omp", "/p");
        close(&c, false).unwrap();
        assert_eq!(
            *host.links.borrow(),
            strings(&[
                "link item-1",
                "unlink item-1",
                "unlink item-1",
                "link item-2",
                "unlink item-2",
                "unlink item-2",
                "link item-3",
                "unlink item-3",
            ])
        );
    }

    #[test]
    fn a_turn_is_seen_only_while_shown_in_the_focused_window() {
        let host = install();
        set(|w| w.settings.notify_turn_finished = true);
        let (id, _) = running(&host, "omp", "/p");
        let mut sequence = 0;
        let mut show = || {
            sequence += 1;
            set_viewport(
                &id,
                snap().presentation_epoch,
                rect(800., 600.),
                true,
                sequence,
            )
            .unwrap()
        };
        let turn = || {
            host.omp(&id, WORKING);
            host.omp(&id, DONE);
            item(&id).unread
        };
        assert!(show());
        assert!(!turn(), "seen on screen");
        assert!(host.notifications.borrow().is_empty());
        host.focused.set(false);
        assert!(turn(), "another window has focus");
        assert_eq!(host.notifications.borrow().len(), 1);
        host.focused.set(true);
        window_changed();
        assert!(!item(&id).unread, "focusing the window acknowledges it");
        sheet_open(OpenableSheet::Settings).unwrap();
        assert!(turn(), "a sheet covers the terminal");
        sheet_close().unwrap();
        assert!(item(&id).unread, "closing the sheet does not show it yet");
        assert!(show());
        assert!(!item(&id).unread, "shown again");
    }

    #[test]
    fn selecting_an_unread_row_marks_it_read() {
        let host = install();
        let (a, _) = running(&host, "omp", "/p");
        running(&host, "omp", "/p");
        host.omp(&a, DONE);
        assert!(item(&a).unread);
        select(&a).unwrap();
        assert!(!item(&a).unread);
        assert_eq!(select("item-9").unwrap_err(), "Item item-9 does not exist.");
    }

    #[test]
    fn a_new_session_unbinds_the_old_conversation_until_it_is_bound() {
        let host = install();
        let id = resume(&saved("omp", "s1", "/p/atlas")).unwrap();
        host.complete(&id);
        host.omp(
            &id,
            r#"{"type":"session","id":"s1","file":"/store/omp/s1.jsonl","cwd":"/p/atlas"}"#,
        );
        assert_eq!(item(&id).conversation.unwrap().id, "s1");
        host.omp(&id, r#"{"type":"model","model":"anthropic/opus"}"#);
        let changed = events(&host, "conversations-changed").len();
        host.omp(
            &id,
            r#"{"type":"session","id":"s2","file":null,"cwd":"/p/atlas"}"#,
        );
        let row = item(&id);
        assert_eq!(row.conversation, None);
        assert_eq!(row.title, "OMP");
        assert!(events(&host, "conversations-changed").len() > changed);
        let mut next = saved("omp", "s2", "/p/atlas");
        next.title = "New chat".into();
        next.model = Some("sonnet".into());
        bind_conversation(&id, "stale", next.clone());
        assert!(item(&id).conversation.is_none());
        bind_conversation(&id, "s2", next);
        let row = item(&id);
        assert_eq!(row.conversation.unwrap().path, "/store/omp/s2.jsonl");
        // The model is OMP's own report, not the session file's.
        assert_eq!(
            (row.title.as_str(), row.model.as_deref()),
            ("New chat", Some("anthropic/opus"))
        );
        // A reconnect re-reports the same session: the binding stays.
        host.omp_on(
            &id,
            2,
            r#"{"type":"session","id":"s2","file":"/store/omp/s2.jsonl","cwd":"/p/atlas"}"#,
        );
        host.omp_on(&id, 2, DONE);
        let row = item(&id);
        assert_eq!(row.activity, Activity::YourTurn);
        assert_eq!(row.conversation.unwrap().id, "s2");
    }

    #[test]
    fn omp_reports_set_the_rows_model_and_name_directly() {
        let host = install();
        let id = resume(&saved("omp", "s1", "/p/atlas")).unwrap();
        host.complete(&id);
        // Until OMP reports, a resumed row shows its saved conversation.
        assert_eq!(
            (item(&id).title.as_str(), item(&id).model.as_deref()),
            ("Saved s1", Some("opus"))
        );
        host.omp(&id, r#"{"type":"name","name":"QUAKE: Modify ammo"}"#);
        host.omp(
            &id,
            r#"{"type":"model","model":"openai-codex/gpt-6.1-sol"}"#,
        );
        let row = item(&id);
        assert_eq!(
            (row.title.as_str(), row.model.as_deref()),
            ("QUAKE: Modify ammo", Some("openai-codex/gpt-6.1-sol"))
        );
        assert_eq!(window_title(&snap()), "QUAKE: Modify ammo — atlas");
        let before = host.published.borrow().len();
        host.omp(
            &id,
            r#"{"type":"model","model":"openai-codex/gpt-6.1-sol"}"#,
        );
        assert_eq!(host.published.borrow().len(), before, "unchanged model");
        host.omp(&id, r#"{"type":"name","name":null}"#);
        host.omp(&id, r#"{"type":"model","model":null}"#);
        let row = item(&id);
        assert_eq!((row.title.as_str(), row.model), ("Saved s1", None));
    }

    #[test]
    fn discovery_refreshes_never_replace_a_model_omp_reported() {
        let host = install();
        let id = resume(&saved("omp", "s1", "/p/atlas")).unwrap();
        host.complete(&id);
        // Before OMP reports, discovery keeps the saved model current.
        let mut refreshed = saved("omp", "s1", "/p/atlas");
        refreshed.model = Some("sonnet".into());
        apply_discovery(vec![refreshed.clone()], vec![]);
        assert_eq!(item(&id).model.as_deref(), Some("sonnet"));

        host.omp(&id, r#"{"type":"model","model":"anthropic/opus-live"}"#);
        refreshed.title = "Renamed".into();
        refreshed.model = Some("stale".into());
        apply_discovery(vec![refreshed.clone()], vec![]);
        let row = item(&id);
        assert_eq!(
            (row.title.as_str(), row.model.as_deref()),
            ("Renamed", Some("anthropic/opus-live")),
            "the title still follows discovery"
        );
        // A reported "no model" is live too.
        host.omp(&id, r#"{"type":"model","model":null}"#);
        refreshed.updated_at_ms += 1;
        apply_discovery(vec![refreshed], vec![]);
        assert_eq!(item(&id).model, None);
    }

    #[test]
    fn terminal_titles_update_rows_and_unknown_sessions_are_ignored() {
        let host = install();
        let (id, key) = running(&host, "shell", "/p/atlas");
        host.retitle(key, "  vim notes ");
        assert_eq!(item(&id).title, "vim notes");
        assert_eq!(window_title(&snap()), "vim notes — atlas");
        let before = host.published.borrow().len();
        let state = host_state(&host, key);
        deliver(key + 100, embed::HostEvent::State(&state));
        assert_eq!(host.published.borrow().len(), before);
        host.retitle(key, "");
        assert_eq!(
            item(&id).title,
            "Shell",
            "empty titles fall back to the tool"
        );
    }
    fn host_state(host: &FakeHost, key: SessionKey) -> TerminalState {
        host.state(key).unwrap()
    }

    #[test]
    fn window_title_names_the_active_row_and_its_project() {
        install();
        assert_eq!(window_title(&snap()), "Ultra 9000");
    }

    #[test]
    fn foreground_jobs_publish_after_two_polls_and_ignore_the_shell_itself() {
        let host = install();
        let (s, key) = running(&host, "shell", "/p");
        let (agent, agent_key) = running(&host, "omp", "/p");
        host.set_job(key, Some((900, "sleep")));
        host.set_job(agent_key, Some((901, "rg")));
        poll_jobs();
        assert_eq!(item(&s).foreground_job, None);
        poll_jobs();
        assert_eq!(item(&s).foreground_job.unwrap().command, "sleep");
        assert_eq!(item(&agent).foreground_job, None, "only shells are polled");
        host.set_job(key, Some((1000 + key as i32, "zsh")));
        poll_jobs();
        assert_eq!(item(&s).foreground_job, None, "back at the prompt");
    }

    #[test]
    fn viewport_reports_apply_in_order_for_the_active_row_only() {
        let host = install();
        let (a, _) = running(&host, "shell", "/p");
        let (b, kb) = running(&host, "shell", "/p");
        let epoch = snap().presentation_epoch;
        assert_eq!(
            set_viewport(&b, epoch, rect(f64::NAN, 1.), true, 1).unwrap_err(),
            "viewport values must be finite"
        );
        assert_eq!(
            set_viewport(&b, epoch, rect(-1., 1.), true, 1).unwrap_err(),
            "viewport size must not be negative"
        );
        assert!(!set_viewport(&a, epoch, rect(9., 9.), true, 1).unwrap());
        assert!(!set_viewport("item-77", epoch, rect(9., 9.), true, 1).unwrap());
        assert!(!set_viewport(&b, epoch - 1, rect(9., 9.), true, 1).unwrap());
        assert!(set_viewport(&b, epoch, rect(9., 9.), true, 5).unwrap());
        assert!(!set_viewport(&b, epoch, rect(9., 9.), true, 5).unwrap());
        assert!(!set_viewport(&b, epoch, rect(9., 9.), true, 4).unwrap());
        assert!(!focus(&b, epoch, 4).unwrap(), "not the applied report");
        assert!(!focus(&a, epoch, 5).unwrap());
        assert!(focus(&b, epoch, 5).unwrap());
        assert_eq!(host.calls_of("focus"), vec![kb]);
        invalidate_focus();
        assert!(!focus(&b, epoch, 5).unwrap());
        assert!(set_viewport(&b, epoch, rect(9., 9.), true, 6).unwrap());
        dispatch(Action::FocusSidebar).unwrap();
        assert!(host.calls.borrow().contains(&"focus-sidebar".to_string()));
        assert!(!focus(&b, epoch, 6).unwrap(), "the sidebar took focus");
        assert!(set_viewport(&b, epoch, rect(9., 9.), true, 7).unwrap());
        host.presentable.set(false);
        assert!(!focus(&b, epoch, 7).unwrap(), "minimized window");
        window_changed();
        host.presentable.set(true);
        assert!(!focus(&b, epoch, 7).unwrap(), "report dropped while hidden");
        // A hidden report never makes the terminal focusable.
        assert!(set_viewport(&b, epoch, rect(9., 9.), false, 8).unwrap());
        assert!(!focus(&b, epoch, 8).unwrap());
        sheet_open(OpenableSheet::Shortcuts).unwrap();
        let covered = snap().presentation_epoch;
        assert!(!set_viewport(&b, covered, rect(9., 9.), true, 9).unwrap());
        assert!(!focus(&b, covered, 8).unwrap());
        // A row without a session yet has nothing to place or focus.
        let starting = launch("shell", "/p");
        let epoch = snap().presentation_epoch;
        assert!(!set_viewport(&starting, epoch, rect(9., 9.), true, 10).unwrap());
        assert!(!focus(&starting, epoch, 10).unwrap());
    }

    #[test]
    fn item_and_project_navigation_follow_the_sidebar_order() {
        let host = install();
        let (a, _) = running(&host, "shell", "/p/one");
        let (b, _) = running(&host, "shell", "/p/two");
        let (c, _) = running(&host, "shell", "/p/one");
        let go = |action| {
            dispatch(action).unwrap();
            active().unwrap()
        };
        assert_eq!(go(Action::Item(2)), c);
        assert_eq!(go(Action::Item(3)), b);
        assert_eq!(go(Action::Item(4)), b, "no fourth row");
        assert_eq!(go(Action::Item(0)), b);
        assert_eq!(go(Action::ItemNext), a, "wraps");
        assert_eq!(go(Action::ItemPrevious), b, "wraps back");
        assert_eq!(go(Action::ProjectNext), a);
        assert_eq!(go(Action::ProjectPrevious), b);
        ui_set(None, Some(strings(&["/p/one"]))).unwrap();
        assert_eq!(go(Action::Item(1)), b, "folded rows are not numbered");
        assert_eq!(go(Action::ItemNext), b);
    }

    #[test]
    fn with_nothing_selected_navigation_starts_from_the_ends() {
        let host = install();
        dispatch(Action::JumpWaiting).unwrap();
        dispatch(Action::ItemNext).unwrap();
        assert_eq!(active(), None);
        let failed = launch("shell", "/p");
        host.fail(&failed, "no pty");
        let (a, ka) = running(&host, "shell", "/p");
        let (b, kb) = running(&host, "shell", "/p");
        for (id, key) in [(a, ka), (b, kb)] {
            close(&id, false).unwrap();
            host.exit(key, None);
        }
        assert_eq!(active(), None, "a failed row is no fallback");
        dispatch(Action::ItemPrevious).unwrap();
        assert_eq!(active(), Some(failed));
    }

    #[test]
    fn jump_waiting_visits_questions_first_then_unseen_turns() {
        let host = install();
        let (a, _) = running(&host, "omp", "/p");
        let (b, _) = running(&host, "omp", "/p");
        let (c, _) = running(&host, "omp", "/p");
        host.omp(&a, DONE);
        host.omp(&b, APPROVAL);
        // An agent on a background job is not waiting on the user.
        host.omp(&c, &waiting(&[("bun run test", 1)]));
        let jump = || {
            dispatch(Action::JumpWaiting).unwrap();
            active().unwrap()
        };
        assert_eq!(jump(), b);
        assert_eq!(jump(), a);
        assert!(!item(&a).unread);
        assert_eq!(jump(), b, "the visited turn stays in the cycle");
        assert_eq!(jump(), a);
        select(&c).unwrap();
        assert_eq!(jump(), b);
        assert_eq!(jump(), b, "a read turn drops out once the cycle restarts");
    }

    #[test]
    fn sheets_cover_the_terminal_and_palette_scope_follows_the_active_row() {
        let host = install();
        let (_, key) = running(&host, "shell", "/p/atlas");
        let epoch = snap().presentation_epoch;
        dispatch(Action::PaletteProject).unwrap();
        assert_eq!(
            sheet(),
            json!({"kind":"palette","projectId":"/p/atlas","mode":"open"})
        );
        assert_eq!(snap().presentation_epoch, epoch + 1);
        assert_eq!(host.calls_of("hide"), vec![key]);
        dispatch(Action::Palette).unwrap();
        assert_eq!(sheet()["projectId"], serde_json::Value::Null);
        dispatch(Action::Settings).unwrap();
        assert_eq!(sheet(), json!({"kind":"settings"}));
        dispatch(Action::Shortcuts).unwrap();
        assert_eq!(sheet(), json!({"kind":"shortcuts"}));
        sheet_open(OpenableSheet::Palette {
            project_id: Some("/p/x".into()),
            mode: PaletteMode::New,
        })
        .unwrap();
        assert_eq!(
            sheet(),
            json!({"kind":"palette","projectId":"/p/x","mode":"new"})
        );
        sheet_close().unwrap();
        assert_eq!(
            (snap().view, sheet()),
            ("terminal", serde_json::Value::Null)
        );
        assert_eq!(host.calls_of("hide"), vec![key], "a sheet hides it once");
    }

    #[test]
    fn quitting_records_running_conversations_and_blocks_launches() {
        let host = install();
        let agent = resume(&saved("claude", "s1", "/p")).unwrap();
        host.complete(&agent);
        let (shell, _) = running(&host, "shell", "/p");
        let starting = launch("omp", "/p");
        let (_, gone) = running(&host, "shell", "/p");
        host.exit(gone, Some(5));
        let live = vec![agent.clone(), shell.clone(), starting.clone()];
        assert_eq!(live_items().unwrap(), live);
        open_quit().unwrap();
        assert_eq!(sheet(), json!({"kind":"quit","itemIds":live}));
        close(&starting, false).unwrap();
        assert_eq!(sheet()["itemIds"], json!([agent, shell]));
        let ui = serde_json::to_value(quit_snapshot().unwrap()).unwrap();
        assert_eq!(
            ui["lastSet"],
            json!([{"tool":"claude","title":"Saved s1","projectId":"/p","cwd":"/p",
                "conversationId":"s1","conversationPath":"/store/claude/s1.jsonl"}])
        );
        assert_eq!(
            launch_result("shell").unwrap_err(),
            "The application is quitting."
        );
        cancel_quitting();
        assert!(launch_result("shell").is_ok());
        assert_eq!(last_set_entries().unwrap().len(), 1);
        let entry = serde_json::to_value(snap().last_set).unwrap();
        assert_eq!(entry[0]["conversation"]["id"], "s1");
        let ui = last_set_dismiss().unwrap();
        assert!(ui.last_set.is_empty());
        assert!(snap().last_set.is_none());
        shutdown_all();
        assert!(launch_result("shell").is_err());
    }
    fn launch_result(tool: &str) -> Result<String, String> {
        begin_launch(
            tool.into(),
            "/p".into(),
            "/p".into(),
            None,
            None,
            None,
            None,
        )
    }

    #[test]
    fn ui_state_changes_are_published_and_returned_for_saving() {
        install();
        let ui = ui_set(Some(false), Some(strings(&["/p"]))).unwrap();
        assert!(!ui.sidebar_expanded);
        assert_eq!(snap().ui.folded_project_ids, strings(&["/p"]));
        let ui = ui_set(None, None).unwrap();
        assert_eq!(
            (ui.sidebar_expanded, ui.folded_projects),
            (false, strings(&["/p"]))
        );
        dispatch(Action::ToggleSidebar).unwrap();
        assert!(snap().ui.sidebar_expanded);
        load_ui(UiStateFile {
            sidebar_expanded: false,
            folded_projects: vec![],
            last_set: vec![ui_state::LastSetEntry {
                tool: "omp".into(),
                title: "Saved".into(),
                project_id: "/p".into(),
                cwd: "/p".into(),
                conversation_id: "s1".into(),
                conversation_path: "/s1".into(),
            }],
        });
        let s = snap();
        assert!(!s.ui.sidebar_expanded && s.ui.folded_project_ids.is_empty());
        assert_eq!(s.last_set.unwrap()[0].conversation.path, "/s1");
    }

    #[test]
    fn notices_keep_the_latest_five_and_dismiss_by_id() {
        install();
        for n in 1..=7 {
            action_error(format!("failure {n}"));
        }
        let notices = snap().notices;
        let ids: Vec<_> = notices.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            ["notice-3", "notice-4", "notice-5", "notice-6", "notice-7"]
        );
        assert_eq!(notices[0].body, "failure 3");
        let before = revision();
        notice_dismiss("notice-1").unwrap();
        assert_eq!(revision(), before);
        notice_dismiss("notice-5").unwrap();
        assert_eq!(snap().notices.len(), 4);
        assert_eq!(revision(), before + 1);
    }

    #[test]
    fn discovery_refreshes_bound_rows_and_filters_the_palette_list() {
        let host = install();
        let list = conversations_list(None, None, None).unwrap();
        assert!(list.scanning && list.conversations.is_empty());
        apply_discovery(vec![], vec![]);
        assert!(!conversations_list(None, None, None).unwrap().scanning);
        let changed = events(&host, "conversations-changed");
        apply_discovery(vec![], vec![]);
        assert_eq!(
            events(&host, "conversations-changed"),
            changed,
            "no-op rescan"
        );
        let mut old = saved("omp", "old", "/p/atlas");
        old.title = "Fix login bug".into();
        old.model = None;
        let mut fresh = saved("claude", "fresh", "/p/zeta");
        fresh.title = "Refactor".into();
        fresh.updated_at_ms = now() as i64;
        fresh.model = Some("sonnet-4".into());
        let bound = saved("omp", "bound", "/p/atlas");
        let id = resume(&bound).unwrap();
        let mut renamed = bound.clone();
        renamed.title = "New title".into();
        renamed.model = Some("opus-5".into());
        let error = StoreError {
            tool: "codex".into(),
            path: "/store/codex".into(),
            message: "permission denied".into(),
        };
        apply_discovery(vec![old, fresh, renamed], vec![error.clone()]);
        let row = item(&id);
        assert_eq!(
            (row.title.as_str(), row.model.as_deref()),
            ("New title", Some("opus-5"))
        );
        let list = conversations_list(None, None, None).unwrap();
        assert_eq!(list.errors, vec![error]);
        let last_change = events(&host, "conversations-changed").pop().unwrap();
        assert_eq!(json!(list.revision), last_change["revision"]);
        let flags: Vec<_> = list
            .conversations
            .iter()
            .map(|c| {
                (
                    c.id.as_str(),
                    c.running_item_id.as_deref(),
                    c.maybe_open_elsewhere,
                )
            })
            .collect();
        assert_eq!(
            flags,
            vec![
                ("old", None, false),
                ("fresh", None, true),
                ("bound", Some(id.as_str()), false)
            ]
        );
        let found = |project: Option<&str>, query: Option<&str>, limit| {
            conversations_list(project.map(Into::into), query.map(Into::into), limit)
                .unwrap()
                .conversations
                .into_iter()
                .map(|c| c.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(found(None, Some("LOGIN"), None), strings(&["old"]));
        assert_eq!(found(None, Some("zeta"), None), strings(&["fresh"]));
        assert_eq!(found(None, Some("sonnet"), None), strings(&["fresh"]));
        assert_eq!(
            found(Some("/p/atlas"), None, None),
            strings(&["old", "bound"])
        );
        assert_eq!(found(None, None, Some(1)), strings(&["old"]));
    }

    #[test]
    fn config_updates_keep_resolved_tools_unless_their_spec_changed() {
        let host = install();
        assert_eq!(
            settings_snapshot().unwrap_err(),
            "Settings are still loading; try again."
        );
        let spec = |id: &str| Settings::default().tool(id).unwrap().clone();
        tool_resolved(spec("omp"), Ok("/usr/bin/omp".into()));
        let mut stale = spec("codex");
        stale.label = "Old codex".into();
        tool_resolved(stale, Ok("/usr/bin/codex".into()));
        tool_resolved(spec("claude"), Err("claude was not found".into()));
        let tool = |id: &str| snap().tools.into_iter().find(|t| t.id == id).unwrap();
        assert_eq!(
            (tool("omp").available, tool("omp").path),
            (Some(true), Some("/usr/bin/omp".into()))
        );
        assert_eq!(tool("codex").available, None, "result for an outdated spec");
        assert_eq!(
            (tool("claude").available, tool("claude").unavailable_reason),
            (Some(false), Some("claude was not found".into()))
        );
        let mut settings = Settings {
            tray_icon: "dark".into(),
            notify_needs_you: false,
            pinned: vec!["/p/pinned".into()],
            ..Settings::default()
        };
        settings
            .tools
            .iter_mut()
            .find(|t| t.id == "claude")
            .unwrap()
            .label = "CC".into();
        apply_config(update(settings, 3, Some("[font]\nsize = 12\n")));
        assert_eq!(tool("omp").available, Some(true));
        assert_eq!(
            tool("claude").available,
            None,
            "changed spec resolves again"
        );
        assert_eq!(tool("claude").label, "CC");
        assert_eq!(terminal_config(), "[font]\nsize = 12\n");
        assert_eq!(settings_snapshot().unwrap().revision, 3);
        assert_eq!(
            events(&host, "settings-state").last().unwrap()["revision"],
            3
        );
        assert_eq!(
            *host.pin_probes.borrow(),
            vec![vec![PathBuf::from("/p/pinned")]]
        );
        assert!(tray_dark_icon().unwrap());
        assert!(!notify_needs_you().unwrap());
        assert_eq!(
            crate::workspace::settings().unwrap().pinned,
            vec![PathBuf::from("/p/pinned")]
        );
        // Unchanged or absent terminal config is not reloaded again.
        let reloads = || {
            host.calls
                .borrow()
                .iter()
                .filter(|c| *c == "reload")
                .count()
        };
        let before = reloads();
        apply_config(update(Settings::default(), 3, Some("[font]\nsize = 12\n")));
        apply_config(update(Settings::default(), 3, None));
        assert_eq!(reloads(), before);
        // A rejected terminal config keeps the last good one.
        *host.reload_error.borrow_mut() = Some("bad key".into());
        apply_config(update(Settings::default(), 4, Some("broken")));
        assert_eq!(terminal_config(), "[font]\nsize = 12\n");
        // Before the host starts, the config is only stored for it.
        set(|w| w.host_error = Some("starting".into()));
        let reloads = host
            .calls
            .borrow()
            .iter()
            .filter(|c| *c == "reload")
            .count();
        apply_config(update(Settings::default(), 5, Some("early")));
        assert_eq!(terminal_config(), "early");
        assert_eq!(
            host.calls
                .borrow()
                .iter()
                .filter(|c| *c == "reload")
                .count(),
            reloads
        );
        apply_settings_state(state(9));
        assert_eq!(settings_snapshot().unwrap().revision, 9);
        assert_eq!(
            events(&host, "settings-state").last().unwrap()["revision"],
            9
        );
    }
    fn state(revision: u64) -> SettingsState {
        SettingsState {
            revision,
            config_dir: "/cfg".into(),
            machine: "test".into(),
            files: vec![],
            values: vec![],
            errors: vec![],
            reloaded_at: 0,
        }
    }
    fn update(settings: Settings, revision: u64, terminal: Option<&str>) -> ConfigUpdate {
        ConfigUpdate {
            settings,
            state: state(revision),
            keys: Keymap::defaults(runtime::PLATFORM),
            terminal: terminal.map(Into::into),
        }
    }
    fn terminal_config() -> String {
        with(|w| Ok(w.terminal_config.clone())).unwrap()
    }

    #[test]
    fn pinned_projects_lead_the_sidebar_and_show_missing_roots() {
        let dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
        let root = dir.path().canonicalize().unwrap();
        let present = root.join("atlas");
        std::fs::create_dir(&present).unwrap();
        let gone = root.join("gone");
        let pinned = vec![present.clone(), gone.clone()];
        let probe = pinned_missing(&pinned);
        let key = |p: &Path| p.to_string_lossy().into_owned();
        assert_eq!(
            probe,
            HashMap::from([(key(&present), false), (key(&gone), true)])
        );
        let host = install();
        set(|w| {
            w.settings.pinned = pinned.clone();
            w.settings.projects.insert(
                present.clone(),
                crate::config::ProjectOverride {
                    name: Some("Atlas".into()),
                    default_tool: Some("claude".into()),
                },
            );
        });
        running(&host, "shell", "/p/other");
        apply_pinned_missing(&pinned, probe);
        let projects: Vec<_> = snap()
            .projects
            .into_iter()
            .map(|p| {
                (
                    p.name,
                    p.pinned,
                    p.missing,
                    p.default_tool,
                    p.item_ids.len(),
                )
            })
            .collect();
        assert_eq!(
            projects,
            vec![
                ("Atlas".into(), true, false, "claude".into(), 0),
                ("gone".into(), true, true, "omp".into(), 0),
                ("other".into(), false, false, "omp".into(), 1),
            ]
        );
        // A probe for an outdated pin list is ignored.
        let before = revision();
        apply_pinned_missing(&pinned[..1], HashMap::from([(key(&present), true)]));
        assert_eq!(revision(), before);
    }

    #[test]
    fn new_item_shortcuts_start_next_to_the_active_row() {
        let host = install();
        let target = |action| with(|w| Ok(w.new_item_target(action))).unwrap();
        assert_eq!(target(Action::NewAgent), ("omp".into(), "/".into(), None));
        set(|w| {
            w.settings.projects.insert(
                "/p/atlas".into(),
                crate::config::ProjectOverride {
                    name: None,
                    default_tool: Some("claude".into()),
                },
            );
        });
        let id = begin_launch(
            "shell".into(),
            "/p/atlas/src".into(),
            "/p/atlas".into(),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        host.complete(&id);
        let anchor = Some(id);
        assert_eq!(
            target(Action::NewAgent),
            ("claude".into(), "/p/atlas".into(), anchor.clone())
        );
        assert_eq!(
            target(Action::NewTerminal),
            ("shell".into(), "/p/atlas/src".into(), anchor.clone())
        );
        assert_eq!(
            target(Action::NewNeovim),
            ("neovim".into(), "/p/atlas/src".into(), anchor)
        );
    }

    #[test]
    fn app_keys_map_through_the_platform_keymap() {
        install();
        let key = |code, named| embed::AppKey {
            ctrl: cfg!(target_os = "linux"),
            shift: cfg!(target_os = "linux"),
            alt: false,
            logo: cfg!(target_os = "macos"),
            code,
            named,
        };
        // `J` is evdev 36 and macOS keycode 38.
        let j = if cfg!(target_os = "macos") { 38 } else { 36 };
        assert_eq!(key_action(&key(j, None)), Some(Action::JumpWaiting));
        assert_eq!(key_action(&key(999, None)), None, "unknown key code");
        let mut page = key(0, Some(embed::NamedKey::PageDown));
        page.shift = false;
        page.logo = false;
        page.ctrl = true;
        let expected = cfg!(target_os = "linux").then_some(Action::ItemNext);
        assert_eq!(key_action(&page), expected);
    }

    #[test]
    fn snapshot_wire_shape_matches_the_frontend_contract() {
        install();
        let s = serde_json::to_value(snap()).unwrap();
        assert_eq!(s["view"], "terminal");
        assert_eq!(s["defaultCwd"], "/");
        assert_eq!(s["hostError"], serde_json::Value::Null);
        assert_eq!(s["lastSet"], serde_json::Value::Null);
        let tools: Vec<_> = s["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(tools, ["omp", "claude", "codex", "pi", "shell", "neovim"]);
        assert_eq!(s["tools"][0]["canResume"], true);
        assert_eq!(s["tools"][4]["canResume"], false);
        let palette = s["shortcuts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["action"] == "palette")
            .unwrap()
            .clone();
        assert!(palette["label"].is_string());
    }
}
