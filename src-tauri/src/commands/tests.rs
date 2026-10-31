use super::test_support::{mock_app, Main};
use super::*;
use crate::{
    config::{ConfigStore, Settings},
    runtime::{test_support::serve, ConfigUpdate, PLATFORM},
    shortcuts::Keymap,
    ui_state::{LastSetEntry, UiStateFile},
    workspace::{
        test_support::{install, FakeHost},
        PaletteMode, Process, Sheet,
    },
};
use std::{
    fs,
    path::Path,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};
use tauri::{test::MockRuntime, Manager};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "u9k-commands-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    /// A directory below this one, created.
    fn sub(&self, relative: &str) -> String {
        let path = self.0.join(relative);
        fs::create_dir_all(&path).unwrap();
        path.to_string_lossy().into_owned()
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
/// The main thread, the served integration worker and a fresh fake-host workspace.
fn setup() -> (Main, Rc<FakeHost>, tauri::App<MockRuntime>) {
    let main = Main::acquire();
    serve();
    main.idle();
    let host = install();
    (main, host, mock_app(true))
}
fn state(app: &tauri::App<MockRuntime>) -> WorkspaceState {
    workspace_state(app.handle().clone()).unwrap()
}
fn apply_settings(settings: Settings) {
    workspace::apply_config(ConfigUpdate {
        settings,
        state: ConfigStore::load(serve().paths.clone()).snapshot(),
        keys: Keymap::defaults(PLATFORM),
        terminal: None,
    });
}
fn conversation(tool: &str, id: &str, cwd: &str) -> Conversation {
    Conversation {
        tool: tool.into(),
        id: id.into(),
        path: PathBuf::from(format!("/store/{id}.jsonl")),
        title: format!("Saved {id}"),
        cwd: cwd.into(),
        cwd_missing: false,
        project_root: cwd.into(),
        created_at_ms: 0,
        updated_at_ms: 0,
        model: None,
    }
}
fn notice_bodies(state: &WorkspaceState) -> Vec<String> {
    state
        .notices
        .iter()
        .map(|n| {
            serde_json::to_value(n).unwrap()["body"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}
fn launch(main: &Main, tool: &str, cwd: &str) -> Result<String, String> {
    main.run(item_launch(tool.into(), cwd.into(), None))
}

#[test]
fn launch_validates_tool_directory_and_roots_before_adding_a_row() {
    let (main, host, app) = setup();
    let dir = Dir::new();
    let root = dir.sub("root");
    let project = dir.sub("root/atlas");
    fs::create_dir(Path::new(&project).join(".git")).unwrap();
    let nested = dir.sub("root/atlas/src");
    let outside = dir.sub("elsewhere");

    assert_eq!(
        launch(&main, "nope", &nested).unwrap_err(),
        "Unknown tool: nope"
    );
    let missing = dir.0.join("missing");
    assert_eq!(
        launch(&main, "shell", missing.to_str().unwrap()).unwrap_err(),
        format!("{} does not exist.", missing.display())
    );
    apply_settings(Settings {
        roots: vec![PathBuf::from(&root)],
        ..Settings::default()
    });
    assert_eq!(
        launch(&main, "shell", &outside).unwrap_err(),
        "Directory is outside projects.roots."
    );
    assert!(host.prepared.borrow().is_empty());
    assert!(state(&app).items.is_empty(), "rejected launches add no row");

    let id = launch(&main, "shell", &format!("{nested}/")).unwrap();
    let prepared = host.prepared.borrow().clone();
    assert_eq!(prepared.len(), 1);
    assert_eq!(
        (
            prepared[0].id.as_str(),
            prepared[0].tool.as_str(),
            prepared[0].cwd.as_str()
        ),
        (id.as_str(), "shell", nested.as_str())
    );
    let snapshot = state(&app);
    let item = &snapshot.items[0];
    assert_eq!(item.project_id, project, "the git root owns the item");
    assert_eq!(item.process, Some(Process::Starting));
    assert_eq!(snapshot.active_item_id.as_deref(), Some(id.as_str()));
}

#[test]
fn resume_reuses_the_running_item_and_rejects_unresumable_tools() {
    let (main, host, app) = setup();
    let dir = Dir::new();
    let cwd = dir.sub("atlas");
    let saved = conversation("omp", "native-c1", &cwd);
    let path = saved.path.to_string_lossy().into_owned();

    assert_eq!(
        main.run(item_resume("nope".into(), "native-c1".into(), path.clone()))
            .unwrap_err(),
        "Unknown tool: nope"
    );
    assert_eq!(
        main.run(item_resume(
            "shell".into(),
            "native-c1".into(),
            path.clone()
        ))
        .unwrap_err(),
        "This tool has no saved-conversation store."
    );
    assert_eq!(
        main.run(launch_item(
            "shell".into(),
            cwd.clone(),
            None,
            Some(saved.clone())
        ))
        .unwrap_err(),
        "This tool cannot resume conversations."
    );

    let id = main
        .run(launch_item("omp".into(), cwd.clone(), None, Some(saved)))
        .unwrap();
    assert_eq!(host.prepared.borrow()[0].args, ["--resume", path.as_str()]);
    let other = launch(&main, "shell", &cwd).unwrap();
    assert_eq!(state(&app).active_item_id, Some(other));

    // Resuming a conversation that is already running selects it instead of a second copy.
    let resumed = main
        .run(item_resume("omp".into(), "native-c1".into(), path))
        .unwrap();
    assert_eq!(resumed, id);
    assert_eq!(host.prepared.borrow().len(), 2);
    assert_eq!(state(&app).active_item_id, Some(id));
}

#[test]
fn restart_replaces_only_ended_items_in_a_still_valid_directory() {
    let (main, host, app) = setup();
    let dir = Dir::new();
    let cwd = dir.sub("work");
    let id = launch(&main, "shell", &cwd).unwrap();
    let key = host.complete(&id);
    assert_eq!(
        main.run(item_restart(id.clone())).unwrap_err(),
        "Only exited or failed items can restart."
    );
    host.exit(key, Some(2));
    assert_eq!(state(&app).items[0].process, Some(Process::Exited));

    let restarted = main.run(item_restart(id.clone())).unwrap();
    assert_ne!(restarted, id);
    let snapshot = state(&app);
    assert_eq!(
        snapshot.items.len(),
        1,
        "the ended row is replaced in place"
    );
    assert_eq!(snapshot.items[0].id, restarted);
    assert_eq!(host.calls_of("remove"), [key]);
    assert_eq!(host.prepared.borrow().last().unwrap().cwd, cwd);

    let key = host.complete(&restarted);
    host.exit(key, Some(2));
    fs::remove_dir_all(&cwd).unwrap();
    assert_eq!(
        main.run(item_restart(restarted.clone())).unwrap_err(),
        format!("{cwd} does not exist.")
    );
    assert_eq!(state(&app).items[0].id, restarted);
    assert!(main.run(item_restart("item-404".into())).is_err());
}

#[test]
fn close_asks_before_stopping_a_running_agent() {
    let (main, host, app) = setup();
    let dir = Dir::new();
    let cwd = dir.sub("atlas");
    let starting = launch(&main, "shell", &cwd).unwrap();
    assert_eq!(main.run(item_close(starting, false)).unwrap(), "closed");
    assert!(
        state(&app).items.is_empty(),
        "a starting row closes at once"
    );

    let agent = launch(&main, "omp", &cwd).unwrap();
    let key = host.complete(&agent);
    host.omp(&agent, r#"{"type":"state","state":"working"}"#);
    assert_eq!(
        main.run(item_close(agent.clone(), false)).unwrap(),
        "confirm"
    );
    assert!(matches!(
        state(&app).sheet,
        Some(Sheet::ConfirmClose { ref item_id, .. }) if *item_id == agent
    ));
    assert!(host.calls_of("stop").is_empty());
    assert_eq!(main.run(item_close(agent, true)).unwrap(), "closed");
    assert_eq!(host.calls_of("stop"), [key]);
    assert!(state(&app).sheet.is_none());
}

#[test]
fn last_set_resume_reports_failures_skips_running_and_clears_the_set() {
    let (main, host, app) = setup();
    let dir = Dir::new();
    let cwd = dir.sub("atlas");
    let running = conversation("omp", "native-running", &cwd);
    let id = main
        .run(launch_item(
            "omp".into(),
            cwd.clone(),
            None,
            Some(running.clone()),
        ))
        .unwrap();
    let shell = launch(&main, "shell", &cwd).unwrap();
    assert_eq!(state(&app).active_item_id.as_deref(), Some(shell.as_str()));
    let entry = |tool: &str, id: &str, path: &str| LastSetEntry {
        kind: crate::ui_state::LastSetKind::Agent,
        path: None,
        tool: tool.into(),
        title: "Saved".into(),
        project_id: cwd.clone(),
        cwd: cwd.clone(),
        conversation_id: id.into(),
        conversation_path: path.into(),
    };
    let gone = dir.0.join("gone.jsonl");
    workspace::load_ui(UiStateFile {
        last_set: vec![
            entry("nope", "a", "/store/a.jsonl"),
            entry("shell", "b", "/store/b.jsonl"),
            entry("omp", "native-gone", gone.to_str().unwrap()),
            entry("omp", "native-running", &running.path.to_string_lossy()),
        ],
        ..Default::default()
    });

    main.run(last_set_resume()).unwrap();
    let snapshot = state(&app);
    let bodies = notice_bodies(&snapshot);
    assert!(
        bodies.contains(&"Unknown tool: nope".to_owned()),
        "{bodies:?}"
    );
    assert!(bodies.contains(&"This tool has no saved-conversation store.".to_owned()));
    assert!(bodies
        .iter()
        .any(|b| b.starts_with("Cannot read saved conversation: ")));
    assert_eq!(host.prepared.borrow().len(), 2, "nothing new was launched");
    assert_eq!(
        snapshot.active_item_id,
        Some(id),
        "the running entry is selected"
    );
    assert!(workspace::last_set_entries().unwrap().is_empty());
    assert!(snapshot.last_set.is_none());
}

#[test]
fn quit_records_running_agents_refuses_new_launches_and_exits() {
    let (main, host, _app) = setup();
    let dir = Dir::new();
    let cwd = dir.sub("atlas");
    let agent = main
        .run(launch_item(
            "omp".into(),
            cwd.clone(),
            None,
            Some(conversation("omp", "native-quit", &cwd)),
        ))
        .unwrap();
    host.complete(&agent);
    let shell = launch(&main, "shell", &cwd).unwrap();
    host.complete(&shell);

    let (exited, exit) = mpsc::channel();
    main.run(quit(move || exited.send(()).unwrap())).unwrap();
    exit.try_recv()
        .expect("the app exits after a successful save");
    let last = workspace::last_set_entries().unwrap();
    assert_eq!(
        last.len(),
        1,
        "only agents with a conversation are resumable"
    );
    assert_eq!(
        (last[0].tool.as_str(), last[0].conversation_id.as_str()),
        ("omp", "native-quit")
    );
    assert_eq!(
        launch(&main, "shell", &cwd).unwrap_err(),
        "The application is quitting."
    );
}

#[test]
fn ui_and_settings_changes_persist_through_the_worker() {
    let (main, _host, app) = setup();
    assert_eq!(
        settings_state(app.handle().clone()).unwrap_err(),
        "Settings are still loading; try again."
    );

    main.run(ui_set(Some(false), Some(vec!["/atlas".into()])))
        .unwrap();
    let ui = serde_json::to_value(state(&app).ui).unwrap();
    assert_eq!(ui["sidebarExpanded"], false);
    assert_eq!(ui["foldedProjectIds"], serde_json::json!(["/atlas"]));

    main.run(settings_set(
        "sidebar.width".into(),
        333.into(),
        WriteTarget::Auto,
    ))
    .unwrap();
    assert_eq!(workspace::settings().unwrap().sidebar_width, 333);
    let snapshot = settings_state(app.handle().clone()).unwrap();
    assert!(snapshot
        .values
        .iter()
        .any(|v| v.key == "sidebar.width" && v.value == 333));
    assert_eq!(
        main.run(settings_set(
            "tools.omp.command".into(),
            "rm".into(),
            WriteTarget::Auto
        ))
        .unwrap_err(),
        "setting is not UI-writable"
    );
    main.run(settings_reset("sidebar.width".into(), WriteTarget::Auto))
        .unwrap();
    assert_eq!(workspace::settings().unwrap().sidebar_width, 304);
}

#[test]
fn config_files_open_in_neovim_in_the_config_project() {
    let (main, host, app) = setup();
    let paths = &serve().paths;
    let keys = paths.keys.to_string_lossy().into_owned();
    main.run(settings_open_file(keys.clone(), Some(0))).unwrap();
    assert!(paths.keys.is_file(), "a starter file is created first");
    let prepared = host.prepared.borrow().last().cloned().unwrap();
    let dir = paths.dir.to_string_lossy();
    assert_eq!(
        (prepared.tool.as_str(), prepared.cwd.as_str()),
        ("neovim", &*dir)
    );
    assert_eq!(prepared.args, ["+1", keys.as_str()], "lines start at 1");
    assert_eq!(state(&app).items[0].project_id, dir);

    main.run(settings_open_file(keys.clone(), None)).unwrap();
    assert_eq!(host.prepared.borrow().last().unwrap().args, [keys]);
    assert_eq!(
        main.run(settings_open_file("/etc/hosts".into(), Some(3)))
            .unwrap_err(),
        "not an app configuration file"
    );
    assert_eq!(host.prepared.borrow().len(), 2);
}

#[test]
fn folder_picker_refuses_to_nest_inside_another_modal() {
    let (main, _host, _app) = setup();
    let _open = ModalGuard::acquire().unwrap();
    assert_eq!(
        main.run(pick_directory("/".into())).unwrap_err(),
        "Another native dialog is open."
    );
    let check = main.run(path_check("/".into())).unwrap();
    assert_eq!(check.normalized.as_deref(), Some("/"));
}

#[test]
fn frontend_ipc_selects_presents_and_dismisses() {
    let (main, host, app) = setup();
    let dir = Dir::new();
    let cwd = dir.sub("atlas");
    let handle = || app.handle().clone();
    let a = launch(&main, "shell", &cwd).unwrap();
    let b = launch(&main, "shell", &cwd).unwrap();
    let key_a = host.complete(&a);
    host.complete(&b);

    assert!(item_select(handle(), "item-404".into()).is_err());
    item_select(handle(), a.clone()).unwrap();
    assert_eq!(state(&app).active_item_id, Some(a.clone()));

    let rect: ViewportRectDto = serde_json::from_value(serde_json::json!({
        "x": 240.0, "y": 0.0, "width": 800.0, "height": 600.0,
        "topLeftRadius": 8.0, "bottomLeftRadius": 8.0
    }))
    .unwrap();
    let epoch = state(&app).presentation_epoch;
    assert!(terminal_set_viewport(handle(), a.clone(), epoch, rect, true, 1).unwrap());
    assert!(host
        .calls
        .borrow()
        .contains(&format!("viewport {key_a} true")));
    assert!(!terminal_set_viewport(handle(), a.clone(), epoch - 1, rect, true, 2).unwrap());
    assert!(terminal_focus(handle(), a.clone(), epoch, 1).unwrap());
    assert_eq!(host.calls_of("focus"), [key_a]);
    let mut bad = rect;
    bad.width = f64::NAN;
    assert_eq!(
        terminal_set_viewport(handle(), a.clone(), epoch, bad, true, 3).unwrap_err(),
        "viewport values must be finite"
    );

    sheet_open(
        handle(),
        OpenableSheet::Palette {
            project_id: None,
            mode: PaletteMode::New,
            focus: None,
        },
    )
    .unwrap();
    assert!(matches!(
        state(&app).sheet,
        Some(Sheet::Palette {
            mode: PaletteMode::New,
            ..
        })
    ));
    assert!(
        !terminal_focus(handle(), a.clone(), state(&app).presentation_epoch, 3).unwrap(),
        "a sheet covers the terminal"
    );
    sheet_close(handle()).unwrap();
    assert!(state(&app).sheet.is_none());

    workspace::action_error("boom".into());
    let notice = serde_json::to_value(&state(&app).notices[0]).unwrap();
    assert_eq!(notice["body"], "boom");
    notice_dismiss(handle(), notice["id"].as_str().unwrap().into()).unwrap();
    assert!(state(&app).notices.is_empty());
}

#[test]
fn conversation_list_filters_saved_conversations_and_marks_running_ones() {
    let (main, _host, app) = setup();
    let dir = Dir::new();
    let cwd = dir.sub("atlas");
    let running = conversation("omp", "native-list-1", &cwd);
    let mut other = conversation("omp", "native-list-2", &cwd);
    other.title = "Refactor parser".into();
    workspace::apply_discovery(vec![running.clone(), other], vec![]);
    let id = main
        .run(launch_item("omp".into(), cwd, None, Some(running)))
        .unwrap();

    let list = conversations_list(app.handle().clone(), None, None, Some(1)).unwrap();
    assert_eq!(list.conversations.len(), 1, "limit applies");
    let list = conversations_list(app.handle().clone(), None, Some("PARSER".into()), None).unwrap();
    assert_eq!(list.conversations.len(), 1);
    assert_eq!(list.conversations[0].id, "native-list-2");
    let list = conversations_list(app.handle().clone(), None, Some("saved".into()), None).unwrap();
    let entry = serde_json::to_value(&list.conversations[0]).unwrap();
    assert_eq!(entry["id"], "native-list-1");
    assert_eq!(entry["runningItemId"], id);
}

#[test]
fn hiding_and_quitting_need_the_main_window() {
    let (main, host, app) = setup();
    let windowless = mock_app(false);
    assert_eq!(
        app_hide(windowless.handle().clone()).unwrap_err(),
        "main window not found"
    );
    app_hide(app.handle().clone()).unwrap();

    let dir = Dir::new();
    let id = launch(&main, "shell", &dir.sub("atlas")).unwrap();
    host.complete(&id);
    app_request_quit(app.handle().clone()).unwrap();
    main.until("the quit sheet", || state(&app).sheet.is_some());
    assert!(matches!(
        state(&app).sheet,
        Some(Sheet::Quit { ref item_ids }) if *item_ids == [id.clone()]
    ));
    assert!(app
        .get_webview_window(crate::lifecycle::MAIN_WINDOW)
        .is_some());
}

#[test]
fn documents_resume_from_disk_without_starting_a_process() {
    let (main, host, app) = setup();
    let dir = Dir::new();
    let path = dir.0.join("report.md");
    fs::write(&path, "# Before\n").unwrap();
    let id = main
        .run(open_document(
            path.to_string_lossy().into_owned(),
            true,
            false,
        ))
        .unwrap();
    let saved = workspace::quit_snapshot().unwrap();
    workspace::cancel_quitting();
    workspace::close(&id, false).unwrap();
    fs::write(&path, "# After the application quit\n").unwrap();
    workspace::load_ui(saved);
    main.run(last_set_resume()).unwrap();
    let state = state(&app);
    assert_eq!(state.items.len(), 1);
    assert_eq!(state.view, "document");
    assert!(state.last_set.is_none());
    let document = state.items[0].document.as_ref().unwrap();
    assert_eq!(
        document.size_bytes,
        "# After the application quit\n".len() as u64
    );
    assert!(host.prepared.borrow().is_empty());
}

#[test]
fn palette_editor_route_does_not_create_a_document_row() {
    let (main, host, app) = setup();
    let dir = Dir::new();
    let path = dir.0.join("report.md");
    fs::write(&path, "# Report\n").unwrap();
    let editor = main
        .run(new_neovim_for_path(path.to_string_lossy().into_owned()))
        .unwrap();
    let state = state(&app);
    assert_eq!(state.items.len(), 1);
    assert_eq!(state.items[0].id, editor);
    assert_eq!(state.items[0].tool, "neovim");
    assert!(state.items[0].document.is_none());
    assert_eq!(
        host.prepared.borrow()[0].args,
        [
            "--".to_owned(),
            path.canonicalize().unwrap().to_string_lossy().into_owned()
        ]
    );
}
