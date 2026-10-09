use super::super::tests::Fixture;
use super::*;
use crate::config::schema::{builtins, ProjectOverride};
use serde_json::json;
use std::fs;

fn errors(store: &ConfigStore) -> Vec<SettingsError> {
    store.snapshot().errors
}
fn only_error(store: &ConfigStore) -> SettingsError {
    let errors = errors(store);
    assert_eq!(errors.len(), 1, "{errors:?}");
    errors.into_iter().next().unwrap()
}
fn value(store: &ConfigStore, key: &str) -> Option<SettingValue> {
    store.snapshot().values.into_iter().find(|v| v.key == key)
}
fn changed(r: Reloaded) -> (bool, bool, bool) {
    (r.settings, r.terminal, r.keys)
}

#[test]
fn custom_tools_take_every_configured_field_and_default_the_rest() {
    let f = Fixture::new();
    fs::write(
        &f.0.ultra,
        "[tools.aider]\nlabel='Aider'\nglyph='ai'\naccent='#A1B2C3'\ncommand='aider-bin'\nkind='terminal'\nargs=['--no-git']\nresume=['--restore','{id}']\nsessions='claude'\nstatus='omp'\n[tools.aider.env]\nAIDER_MODE='fast'\n[tools.bare]\nkind='agent'\n",
    )
    .unwrap();
    let store = ConfigStore::load(f.0.clone());
    assert!(errors(&store).is_empty());
    let ids: Vec<_> = store
        .settings()
        .tools
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(
        ids,
        ["omp", "claude", "codex", "pi", "shell", "neovim", "aider", "bare"]
    );
    assert_eq!(
        store.settings().tool("aider").unwrap(),
        &ToolSpec {
            id: "aider".into(),
            label: "Aider".into(),
            glyph: "ai".into(),
            icon: None,
            accent: Some("#a1b2c3".into()),
            kind: ToolKind::Terminal,
            command: Some("aider-bin".into()),
            args: vec!["--no-git".into()],
            resume: Some(vec!["--restore".into(), "{id}".into()]),
            sessions: Some(StoreKind::Claude),
            status: StatusSource::Omp,
            env: vec![("AIDER_MODE".into(), "fast".into())],
        }
    );
    assert_eq!(
        store.settings().tool("bare").unwrap(),
        &ToolSpec {
            id: "bare".into(),
            label: "bare".into(),
            glyph: String::new(),
            icon: None,
            accent: None,
            kind: ToolKind::Agent,
            command: Some("bare".into()),
            args: vec![],
            resume: None,
            sessions: None,
            status: StatusSource::Process,
            env: vec![],
        }
    );
}

#[test]
fn tool_enums_map_each_configured_choice() {
    let f = Fixture::new();
    fs::write(
        &f.0.ultra,
        "[tools.a]\nsessions='codex'\n[tools.b]\nsessions='pi'\n[tools.c]\nsessions='omp'\nstatus='process'\n[tools.shell]\nkind='agent'\n",
    )
    .unwrap();
    let store = ConfigStore::load(f.0.clone());
    let s = store.settings();
    assert_eq!(s.tool("a").unwrap().sessions, Some(StoreKind::Codex));
    assert_eq!(s.tool("b").unwrap().sessions, Some(StoreKind::Pi));
    assert_eq!(s.tool("c").unwrap().sessions, Some(StoreKind::Omp));
    assert_eq!(s.tool("c").unwrap().status, StatusSource::Process);
    assert_eq!(s.tool("shell").unwrap().kind, ToolKind::Agent);
}

#[test]
fn builtin_overrides_keep_unconfigured_fields_and_append_env() {
    let f = Fixture::new();
    fs::write(
        &f.0.ultra,
        "[tools.omp]\nlabel='Oh My Pi'\n[tools.omp.env]\nPI_NOTIFICATIONS='on'\n",
    )
    .unwrap();
    let store = ConfigStore::load(f.0.clone());
    let builtin = builtins().into_iter().find(|t| t.id == "omp").unwrap();
    let omp = store.settings().tool("omp").unwrap();
    assert_eq!(omp.label, "Oh My Pi");
    assert_eq!(omp.glyph, builtin.glyph);
    assert_eq!(omp.resume, builtin.resume);
    assert_eq!(omp.status, StatusSource::Omp);
    assert_eq!(
        omp.env,
        [
            ("WARP_CLI_AGENT_PROTOCOL_VERSION".into(), "1".into()),
            ("PI_NOTIFICATIONS".into(), "on".into())
        ]
    );
    assert_eq!(store.settings().tools.len(), builtins().len());
}

#[test]
fn every_scalar_setting_maps_into_settings() {
    let f = Fixture::new();
    fs::write(
        &f.0.ultra,
        "[sidebar]\ndensity='compact'\nauto_collapse_below=0\n[layout]\ncontext_bar='never'\n[notify]\nneeds_you=false\nturn_finished=true\n[terminals]\nremove_on_clean_exit=false\nconfirm_close_with_foreground_job=false\n[confirm]\nclose_unmonitored_agents=true\n[resume]\nopen_elsewhere_hint_seconds=5\n[projects]\npinned=['/a']\nroots=['/b']\n[projects.\"/p\"]\nname='Named'\n",
    )
    .unwrap();
    let store = ConfigStore::load(f.0.clone());
    assert!(errors(&store).is_empty());
    let expected = Settings {
        sidebar_density: "compact".into(),
        sidebar_auto_collapse_below: 0,
        context_bar: "never".into(),
        notify_needs_you: false,
        notify_turn_finished: true,
        remove_on_clean_exit: false,
        confirm_close_with_foreground_job: false,
        confirm_close_unmonitored_agents: true,
        open_elsewhere_hint_seconds: 5,
        pinned: vec!["/a".into()],
        roots: vec!["/b".into()],
        projects: [(
            PathBuf::from("/p"),
            ProjectOverride {
                name: Some("Named".into()),
                default_tool: None,
            },
        )]
        .into(),
        ..Settings::default()
    };
    assert_eq!(store.settings(), &expected);
    assert_eq!(store.settings().project_name(Path::new("/p")), "Named");
    assert_eq!(
        store.settings().project_default_tool(Path::new("/p")),
        "omp"
    );
}

#[test]
fn font_size_needs_a_source_and_ultra_overrides_terminal_file() {
    let f = Fixture::new();
    let mut store = ConfigStore::load(f.0.clone());
    // The built-in default exists only for the settings UI; it is not an override.
    assert_eq!(store.settings().terminal_font_size, None);
    assert_eq!(store.terminal_toml(), "");
    fs::write(
        &f.0.terminal,
        "[colors]\nanything='goes'\n[font]\nsize = 13\n",
    )
    .unwrap();
    store.reload();
    assert!(errors(&store).is_empty());
    assert_eq!(store.settings().terminal_font_size, Some(13.0));
    let source = value(&store, "terminal.font.size").unwrap().source.unwrap();
    assert_eq!(
        (source.file.as_str(), source.line),
        (f.0.terminal.to_str().unwrap(), 4)
    );
    fs::write(&f.0.ultra, "[terminal.font]\nsize = 14.5\n").unwrap();
    store.reload();
    assert_eq!(store.settings().terminal_font_size, Some(14.5));
    assert_eq!(
        value(&store, "terminal.font.size")
            .unwrap()
            .source
            .unwrap()
            .file,
        f.0.ultra.to_string_lossy()
    );
}

#[test]
fn invalid_terminal_font_rejects_the_whole_terminal_file() {
    let f = Fixture::new();
    let good = "[font]\nsize=13\n[colors.primary]\nbackground='#000000'\n";
    fs::write(&f.0.terminal, good).unwrap();
    let mut store = ConfigStore::load(f.0.clone());
    fs::write(
        &f.0.terminal,
        "[font]\nsize=200\n[colors.primary]\nbackground='#111111'\n",
    )
    .unwrap();
    assert_eq!(changed(store.reload()), (false, false, false));
    assert_eq!(store.terminal_toml(), good);
    assert_eq!(store.settings().terminal_font_size, Some(13.0));
    let e = only_error(&store);
    assert_eq!(e.file, f.0.terminal.to_string_lossy());
    assert_eq!(e.line, Some(2));
    assert_eq!(e.key.as_deref(), Some("terminal.font.size"));
    assert_eq!(e.last_good.as_deref(), Some("13"));
}

#[test]
fn reload_reports_exactly_the_layers_that_changed() {
    let f = Fixture::new();
    let mut store = ConfigStore::load(f.0.clone());
    assert_eq!(changed(store.reload()), (false, false, false));
    fs::write(&f.0.keys, "[bindings]\npalette='Ctrl+K'\n").unwrap();
    assert_eq!(changed(store.reload()), (false, false, true));
    assert_eq!(
        store.keys_toml().unwrap()["bindings"]["palette"].as_str(),
        Some("Ctrl+K")
    );
    fs::write(&f.0.terminal, "[colors.primary]\nbackground='#000000'\n").unwrap();
    assert_eq!(changed(store.reload()), (false, true, false));
    fs::write(&f.0.ultra, "[sidebar]\nwidth=300\n").unwrap();
    assert_eq!(changed(store.reload()), (true, false, false));
    fs::write(&f.0.machine_file, "[terminal.font]\nsize=20\n").unwrap();
    assert_eq!(changed(store.reload()), (true, true, false));
    // Unparseable keys keep the last good bindings.
    fs::write(&f.0.keys, "[bindings").unwrap();
    assert_eq!(changed(store.reload()), (false, false, false));
    assert!(store.keys_toml().unwrap().contains_key("bindings"));
    assert_eq!(only_error(&store).file, f.0.keys.to_string_lossy());
    fs::remove_file(&f.0.keys).unwrap();
    assert_eq!(changed(store.reload()), (false, false, true));
    assert!(store.keys_toml().is_none());
}

#[test]
fn unknown_launch_default_falls_back_to_previous_then_omp() {
    let f = Fixture::new();
    fs::write(&f.0.ultra, "[launch]\ndefault_tool='nope'\n").unwrap();
    let mut store = ConfigStore::load(f.0.clone());
    assert_eq!(store.settings().default_tool, "omp");
    let e = only_error(&store);
    assert_eq!(
        e,
        SettingsError {
            file: f.0.ultra.to_string_lossy().into(),
            line: Some(2),
            key: Some("launch.default_tool".into()),
            message: "expected a configured tool id".into(),
            last_good: Some("omp".into()),
        }
    );
    assert_eq!(
        value(&store, "launch.default_tool").unwrap().value,
        json!("omp")
    );
    fs::write(&f.0.ultra, "[launch]\ndefault_tool='claude'\n").unwrap();
    store.reload();
    fs::write(&f.0.ultra, "[launch]\ndefault_tool='nope'\n").unwrap();
    store.reload();
    assert_eq!(store.settings().default_tool, "claude");
    assert_eq!(only_error(&store).last_good.as_deref(), Some("claude"));
    // A previous default whose tool definition disappears is not reused.
    fs::write(
        &f.0.ultra,
        "[launch]\ndefault_tool='mine'\n[tools.mine]\ncommand='mine'\n",
    )
    .unwrap();
    store.reload();
    assert_eq!(store.settings().default_tool, "mine");
    fs::write(&f.0.ultra, "[launch]\ndefault_tool='mine'\n").unwrap();
    store.reload();
    assert_eq!(store.settings().default_tool, "omp");
    assert_eq!(only_error(&store).last_good.as_deref(), Some("omp"));
}

#[test]
fn unknown_project_default_falls_back_to_previous_then_launch_default() {
    let f = Fixture::new();
    let project = Path::new("/p");
    fs::write(
        &f.0.ultra,
        "[launch]\ndefault_tool='codex'\n[projects.\"/p\"]\ndefault_tool='nope'\n",
    )
    .unwrap();
    let mut store = ConfigStore::load(f.0.clone());
    assert_eq!(store.settings().project_default_tool(project), "codex");
    let e = only_error(&store);
    assert_eq!(e.key.as_deref(), Some("projects.\"/p\".default_tool"));
    assert_eq!(e.line, Some(4));
    assert_eq!(e.last_good.as_deref(), Some("codex"));
    fs::write(&f.0.ultra, "[projects.\"/p\"]\ndefault_tool='claude'\n").unwrap();
    store.reload();
    fs::write(&f.0.ultra, "[projects.\"/p\"]\ndefault_tool='nope'\n").unwrap();
    store.reload();
    assert_eq!(store.settings().project_default_tool(project), "claude");
    assert_eq!(only_error(&store).last_good.as_deref(), Some("claude"));
    let snapshot = value(&store, "projects.\"/p\".default_tool").unwrap();
    assert_eq!(snapshot.value, json!("claude"));
    // A project default has no built-in value; the UI shows the launch default.
    assert_eq!(snapshot.default_value, json!("omp"));
}

#[test]
fn invalid_values_report_the_merged_or_file_last_good() {
    let f = Fixture::new();
    fs::write(&f.0.ultra, "[sidebar]\nwidth=10\nbogus=1\n").unwrap();
    let mut store = ConfigStore::load(f.0.clone());
    assert_eq!(store.settings().sidebar_width, 304);
    let errors = errors(&store);
    // Errors follow key order within a file.
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].key.as_deref(), Some("sidebar.bogus"));
    assert_eq!(errors[0].message, "unknown key");
    assert_eq!(errors[0].last_good, None);
    assert_eq!(errors[1].key.as_deref(), Some("sidebar.width"));
    assert_eq!(errors[1].message, "expected integer 240–360");
    assert_eq!(errors[1].last_good.as_deref(), Some("304"));
    fs::write(&f.0.machine_file, "[sidebar]\nwidth=300\n").unwrap();
    store.reload();
    fs::write(&f.0.machine_file, "# moved\n[sidebar]\nwidth=1\n").unwrap();
    store.reload();
    assert_eq!(store.settings().sidebar_width, 300);
    let machine_error = store
        .snapshot()
        .errors
        .into_iter()
        .find(|e| e.file == f.0.machine_file.to_string_lossy())
        .unwrap();
    assert_eq!(machine_error.line, Some(3));
    assert_eq!(machine_error.last_good.as_deref(), Some("300"));
    // The retained value keeps pointing at its last good source line.
    let source = value(&store, "sidebar.width").unwrap().source.unwrap();
    assert_eq!(
        (source.file, source.line),
        (f.0.machine_file.to_string_lossy().into(), 2)
    );
}

#[test]
fn unreadable_and_unparseable_files_keep_the_last_good_file() {
    let f = Fixture::new();
    fs::write(&f.0.ultra, "[sidebar]\nwidth=280\n").unwrap();
    let mut store = ConfigStore::load(f.0.clone());
    fs::write(&f.0.ultra, b"[sidebar]\nwidth=\xff\n").unwrap();
    store.reload();
    assert_eq!(store.settings().sidebar_width, 280);
    let e = only_error(&store);
    assert_eq!(e.file, f.0.ultra.to_string_lossy());
    assert_eq!((e.line, e.key, e.last_good), (None, None, None));
    fs::write(&f.0.ultra, "[sidebar]\nwidth=250\n\n[broken").unwrap();
    store.reload();
    assert_eq!(store.settings().sidebar_width, 280);
    let e = only_error(&store);
    assert_eq!(e.line, Some(4));
    assert_eq!(e.message, "unparseable TOML; retaining last good file");
}

#[test]
fn reported_errors_are_deduplicated_until_the_next_reload() {
    let f = Fixture::new();
    let mut store = ConfigStore::load(f.0.clone());
    let revision = store.snapshot().revision;
    let err = SettingsError {
        file: "x".into(),
        line: None,
        key: None,
        message: "boom".into(),
        last_good: None,
    };
    store.report_error(err.clone());
    store.report_error(err.clone());
    assert_eq!(errors(&store), std::slice::from_ref(&err));
    assert_eq!(store.snapshot().revision, revision + 1);
    store.reload();
    assert!(errors(&store).is_empty());
    store.report_error(err.clone());
    assert_eq!(errors(&store), [err]);
}

#[test]
fn snapshot_lists_ui_values_with_defaults_sources_and_files() {
    let f = Fixture::new();
    fs::write(
        &f.0.ultra,
        "[sidebar]\ndensity='compact'\n[projects]\nroots=['/r']\n[tools.x]\ncommand='x'\n",
    )
    .unwrap();
    let store = ConfigStore::load(f.0.clone());
    let snapshot = store.snapshot();
    let keys: Vec<_> = snapshot.values.iter().map(|v| v.key.as_str()).collect();
    let mut expected: Vec<_> = UI_KEYS.to_vec();
    expected.sort_by_key(|k| k.split('.').collect::<Vec<_>>());
    assert_eq!(keys, expected);
    let density = value(&store, "sidebar.density").unwrap();
    assert_eq!(density.value, json!("compact"));
    assert_eq!(density.default_value, json!("comfortable"));
    assert_eq!(density.source.unwrap().line, 2);
    let width = value(&store, "sidebar.width").unwrap();
    assert_eq!((width.value, width.source), (json!(304), None));
    assert_eq!(snapshot.machine, "test");
    assert_eq!(snapshot.config_dir, f.0.dir.to_string_lossy());
    assert!(snapshot.reloaded_at > 0);
    let files: Vec<_> = snapshot
        .files
        .iter()
        .map(|file| (file.name.as_str(), file.exists))
        .collect();
    assert_eq!(
        files,
        [
            ("ultra.toml", true),
            ("terminal.toml", false),
            ("keys.toml", false),
            ("machines/test.toml", false)
        ]
    );
    assert_eq!(snapshot.files[0].path, f.0.ultra.to_string_lossy());
}

#[test]
fn set_writes_the_targeted_file_and_reloads() {
    let f = Fixture::new();
    let mut store = ConfigStore::load(f.0.clone());
    let revision = store.snapshot().revision;
    store
        .set("sidebar.width", json!(300), WriteTarget::Machine)
        .unwrap();
    assert_eq!(store.settings().sidebar_width, 300);
    assert!(store.snapshot().revision > revision);
    assert!(!f.0.ultra.exists());
    // Auto writes where the effective value is defined.
    store
        .set("sidebar.width", json!(320), WriteTarget::Auto)
        .unwrap();
    assert!(!f.0.ultra.exists());
    let machine = fs::read_to_string(&f.0.machine_file).unwrap();
    assert!(machine.contains("width = 320"), "{machine}");
    store
        .set(
            "default_tool",
            json!("pi"),
            WriteTarget::Project("/p".into()),
        )
        .unwrap();
    assert_eq!(store.settings().project_default_tool(Path::new("/p")), "pi");
    assert!(fs::read_to_string(&f.0.ultra)
        .unwrap()
        .contains("[projects.\"/p\"]"));
    store.reset("sidebar.width", WriteTarget::Machine).unwrap();
    assert_eq!(store.settings().sidebar_width, 304);
}

#[test]
fn font_size_writes_follow_its_source_file_and_key_shape() {
    let f = Fixture::new();
    fs::write(&f.0.ultra, "[terminal.font]\nsize = 12\n").unwrap();
    let mut store = ConfigStore::load(f.0.clone());
    store
        .set("terminal.font.size", json!(16), WriteTarget::Auto)
        .unwrap();
    assert!(!f.0.terminal.exists());
    assert_eq!(store.settings().terminal_font_size, Some(16.0));
    assert_eq!(
        fs::read_to_string(&f.0.ultra).unwrap(),
        "[terminal.font]\nsize = 16\n"
    );
    store
        .set("terminal.font.size", json!(18.5), WriteTarget::Machine)
        .unwrap();
    let machine = fs::read_to_string(&f.0.machine_file).unwrap();
    assert!(machine.contains("[terminal.font]"), "{machine}");
    assert_eq!(store.settings().terminal_font_size, Some(18.5));
    assert!(store
        .set("terminal.font.size", json!(3), WriteTarget::Machine)
        .is_err());
    assert_eq!(store.settings().terminal_font_size, Some(18.5));
}

#[test]
fn set_and_reset_reject_bad_keys_targets_and_values_without_writing() {
    let f = Fixture::new();
    let mut store = ConfigStore::load(f.0.clone());
    let project = || WriteTarget::Project("/p".into());
    for (key, value, target, message) in [
        (
            "launch.default_tool",
            json!("nope"),
            WriteTarget::Auto,
            "unknown tool id",
        ),
        (
            "projects.roots",
            json!(["/r"]),
            WriteTarget::Auto,
            "setting is not UI-writable",
        ),
        (
            "tools.omp.label",
            json!("x"),
            WriteTarget::Machine,
            "setting is not UI-writable",
        ),
        (
            "sidebar.width",
            json!(300),
            project(),
            "project target only supports default_tool",
        ),
        (
            "default_tool",
            json!("pi"),
            WriteTarget::Project("relative".into()),
            "project path must be absolute",
        ),
        (
            "projects.\"/q\".default_tool",
            json!("pi"),
            project(),
            "project key does not match target",
        ),
        (
            "a = 0\nb",
            json!(1),
            WriteTarget::Auto,
            "expected one setting key",
        ),
        (
            "x = {}\n#",
            json!(1),
            WriteTarget::Auto,
            "expected one setting key",
        ),
        (
            "sidebar.density",
            json!("tiny"),
            WriteTarget::Auto,
            "expected comfortable | compact",
        ),
    ] {
        assert_eq!(
            store.set(key, value, target.clone()).unwrap_err(),
            message,
            "{key}"
        );
    }
    assert!(store
        .set("sidebar..width", json!(300), WriteTarget::Auto)
        .is_err());
    assert!(store
        .set("sidebar.width", serde_json::Value::Null, WriteTarget::Auto)
        .is_err());
    assert_eq!(
        store
            .reset("projects.roots", WriteTarget::Auto)
            .unwrap_err(),
        "setting is not UI-writable"
    );
    for path in [&f.0.ultra, &f.0.terminal, &f.0.keys, &f.0.machine_file] {
        assert!(!path.exists(), "{}", path.display());
    }
}

#[test]
fn starter_files_are_valid_and_never_overwrite_or_escape() {
    let f = Fixture::new();
    fs::remove_dir(f.0.dir.join("machines")).unwrap();
    let mut store = ConfigStore::load(f.0.clone());
    assert_eq!(
        store
            .ensure_starter(&f.0.dir.join("other.toml"))
            .unwrap_err(),
        "not an app configuration file"
    );
    assert!(!f.0.dir.join("other.toml").exists());
    fs::write(&f.0.keys, "[bindings]\npalette='Ctrl+K'\n").unwrap();
    for path in [&f.0.ultra, &f.0.terminal, &f.0.keys, &f.0.machine_file] {
        store.ensure_starter(path).unwrap();
    }
    assert_eq!(
        fs::read_to_string(&f.0.keys).unwrap(),
        "[bindings]\npalette='Ctrl+K'\n"
    );
    let ultra = fs::read_to_string(&f.0.ultra).unwrap();
    assert!(ultra.starts_with("# Ultra 9000 settings."));
    assert_eq!(fs::read_to_string(&f.0.machine_file).unwrap(), ultra);
    assert!(fs::read_to_string(&f.0.terminal)
        .unwrap()
        .starts_with("# Embedded terminal only"));
    fs::remove_file(&f.0.keys).unwrap();
    store.ensure_starter(&f.0.keys).unwrap();
    store.reload();
    assert!(errors(&store).is_empty(), "{:?}", errors(&store));
    assert_eq!(store.settings(), &Settings::default());
    assert!(store.keys_toml().unwrap()["bindings"]
        .as_table()
        .unwrap()
        .is_empty());
}

#[test]
fn user_icons_default_to_mono_without_fill() {
    let f = Fixture::new();
    fs::write(
        f.0.dir.join("mark.svg"),
        "<svg xmlns='http://www.w3.org/2000/svg'/>",
    )
    .unwrap();
    fs::write(
        &f.0.ultra,
        "[tools.claude]\nicon='mark.svg'\n[providers.openai]\nicon='mark.svg'\n",
    )
    .unwrap();
    let store = ConfigStore::load(f.0.clone());
    let icon = store
        .settings()
        .tool("claude")
        .unwrap()
        .icon
        .clone()
        .unwrap();
    assert_eq!((icon.tint, icon.fill), (IconTint::Mono, false));
    assert!(icon.url.starts_with("data:image/svg+xml"), "{}", icon.url);
    assert_eq!(store.settings().provider_icons["openai"], icon.url);
}
