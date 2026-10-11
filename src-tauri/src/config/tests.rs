use super::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub(super) struct Fixture(pub(super) ConfigPaths);
impl Fixture {
    pub(super) fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "u9k-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(dir.join("machines")).unwrap();
        Self(ConfigPaths {
            ultra: dir.join("ultra.toml"),
            terminal: dir.join("terminal.toml"),
            keys: dir.join("keys.toml"),
            machine_file: dir.join("machines/test.toml"),
            machine: "test".into(),
            state_file: dir.join("state.json"),
            dir,
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0.dir);
    }
}
#[test]
fn precedence_and_project() {
    let f = Fixture::new();
    fs::write(&f.0.ultra,"[launch]\ndefault_tool='claude'\n[sidebar]\nwidth=260\n[projects.\"/project\"]\ndefault_tool='pi'\n").unwrap();
    fs::write(
        &f.0.machine_file,
        "[launch]\ndefault_tool='codex'\n[sidebar]\nwidth=280",
    )
    .unwrap();
    let s = ConfigStore::load(f.0.clone());
    assert_eq!(s.settings().sidebar_width, 280);
    assert_eq!(s.settings().default_tool, "codex");
    assert_eq!(
        s.settings()
            .project_default_tool(&PathBuf::from("/project")),
        "pi"
    );
    assert_eq!(
        s.settings().project_default_tool(&PathBuf::from("/other")),
        "codex"
    );
    assert_eq!(
        s.snapshot()
            .values
            .iter()
            .find(|v| v.key == "sidebar.width")
            .unwrap()
            .source
            .as_ref()
            .unwrap()
            .line,
        4
    );
}
#[test]
fn project_default_reset_removes_machine_definition() {
    let f = Fixture::new();
    fs::write(&f.0.ultra, "[launch]\ndefault_tool='omp'\n").unwrap();
    fs::write(
        &f.0.machine_file,
        "[projects.\"/project\"]\ndefault_tool='claude'\n",
    )
    .unwrap();
    let mut store = ConfigStore::load(f.0.clone());
    assert_eq!(
        store
            .settings()
            .project_default_tool(&PathBuf::from("/project")),
        "claude"
    );
    store
        .reset("projects.\"/project\".default_tool", WriteTarget::Auto)
        .unwrap();
    assert_eq!(
        store
            .settings()
            .project_default_tool(&PathBuf::from("/project")),
        "omp"
    );
    assert!(!fs::read_to_string(&f.0.machine_file)
        .unwrap()
        .contains("default_tool"));
}
#[test]
fn invalid_keeps_last_good_and_line() {
    let f = Fixture::new();
    fs::write(&f.0.ultra, "[sidebar]\nwidth=280\ndensity='compact'").unwrap();
    let mut s = ConfigStore::load(f.0.clone());
    fs::write(
        &f.0.ultra,
        "# hello\n[sidebar]\nwidth=999\ndensity='comfortable'",
    )
    .unwrap();
    s.reload();
    assert_eq!(s.settings().sidebar_width, 280);
    assert_eq!(s.settings().sidebar_density, "comfortable");
    let e = &s.snapshot().errors[0];
    assert_eq!(e.line, Some(3));
    assert_eq!(e.key.as_deref(), Some("sidebar.width"));
    assert_eq!(e.last_good.as_deref(), Some("280"));
    fs::write(&f.0.ultra, "[broken").unwrap();
    s.reload();
    assert_eq!(s.settings().sidebar_width, 280);
    assert_eq!(s.settings().sidebar_density, "comfortable");
    assert!(!s.snapshot().errors.is_empty());
    fs::remove_file(&f.0.ultra).unwrap();
    s.reload();
    assert_eq!(s.settings().sidebar_width, 304);
}
#[test]
fn comment_preserving_set_reset() {
    let f = Fixture::new();
    fs::write(&f.0.ultra,"# heading\n[sidebar]\n# width explanation\nwidth = 248 # inline\n# density explanation\ndensity = 'compact'\n").unwrap();
    let mut s = ConfigStore::load(f.0.clone());
    s.set("sidebar.width", serde_json::json!(280), WriteTarget::Auto)
        .unwrap();
    let t = fs::read_to_string(&f.0.ultra).unwrap();
    for c in [
        "# heading",
        "# width explanation",
        "# inline",
        "# density explanation",
    ] {
        assert!(t.contains(c), "{c}");
    }
    s.reset("sidebar.width", WriteTarget::Auto).unwrap();
    let t = fs::read_to_string(&f.0.ultra).unwrap();
    for c in [
        "# heading",
        "# width explanation",
        "# inline",
        "# density explanation",
    ] {
        assert!(t.contains(c), "{c}: {t}");
    }
    assert_eq!(s.settings().sidebar_width, 304);
    assert_eq!(s.settings().sidebar_density, "compact");
    assert!(s
        .set("sidebar.width", serde_json::json!(500), WriteTarget::Auto)
        .is_err());
}
#[test]
fn terminal_and_wire_targets() {
    let f = Fixture::new();
    let mut s = ConfigStore::load(f.0.clone());
    s.set(
        "terminal.font.size",
        serde_json::json!(17),
        WriteTarget::Auto,
    )
    .unwrap();
    assert_eq!(s.settings().terminal_font_size, Some(17.0));
    s.set(
        "launch.default_tool",
        serde_json::json!("pi"),
        WriteTarget::Project(PathBuf::from("/project")),
    )
    .unwrap();
    assert_eq!(
        s.settings()
            .project_default_tool(&PathBuf::from("/project")),
        "pi"
    );
    assert!(matches!(
        serde_json::from_str::<WriteTarget>("\"auto\"").unwrap(),
        WriteTarget::Auto
    ));
    let wire = serde_json::to_value(s.snapshot()).unwrap();
    assert!(wire.get("configDir").is_some());
    assert!(wire["values"][0].get("defaultValue").is_some());
}
#[test]
fn machine_font_and_invalid_terminal() {
    let f = Fixture::new();
    let mut s = ConfigStore::load(f.0.clone());
    s.set(
        "terminal.font.size",
        serde_json::json!(17),
        WriteTarget::Auto,
    )
    .unwrap();
    s.set(
        "terminal.font.size",
        serde_json::json!(19),
        WriteTarget::Machine,
    )
    .unwrap();
    assert_eq!(s.settings().terminal_font_size, Some(19.0));
    assert!(fs::read_to_string(&f.0.machine_file)
        .unwrap()
        .contains("19"));
    s.reset("terminal.font.size", WriteTarget::Auto).unwrap();
    assert_eq!(s.settings().terminal_font_size, Some(17.0));
    let last = s.terminal_toml().to_owned();
    fs::write(&f.0.terminal, "[font]\nsize=-10").unwrap();
    s.reload();
    assert_eq!(s.terminal_toml(), last);
    assert!(!s.snapshot().errors.is_empty());
}
#[test]
fn canonical_project_targets_and_inline_tables() {
    let f = Fixture::new();
    fs::write(&f.0.ultra, "# keep\nsidebar = { width = 248, density = 'compact' } # inline\nprojects = { \"/project\" = { default_tool = \"omp\", name = \"Atlas\" } }\n").unwrap();
    fs::write(
        &f.0.terminal,
        "font = { size = 15.0, normal = { family = 'Mono' } } # font\n",
    )
    .unwrap();
    let mut s = ConfigStore::load(f.0.clone());
    let key = "projects.\"/project\".default_tool";
    s.set(
        key,
        serde_json::json!("claude"),
        WriteTarget::Project("/project".into()),
    )
    .unwrap();
    assert_eq!(
        s.settings()
            .project_default_tool(PathBuf::from("/project").as_path()),
        "claude"
    );
    assert!(s
        .set(
            key,
            serde_json::json!("pi"),
            WriteTarget::Project("/other".into())
        )
        .is_err());
    s.reset(key, WriteTarget::Project("/project".into()))
        .unwrap();
    assert_eq!(
        s.settings()
            .project_default_tool(PathBuf::from("/project").as_path()),
        "omp"
    );
    s.set("sidebar.width", serde_json::json!(280), WriteTarget::Auto)
        .unwrap();
    assert_eq!(s.settings().sidebar_width, 280);
    s.reset("sidebar.width", WriteTarget::Auto).unwrap();
    assert_eq!(s.settings().sidebar_width, 304);
    s.set(
        "terminal.font.size",
        serde_json::json!(18),
        WriteTarget::Auto,
    )
    .unwrap();
    assert_eq!(s.settings().terminal_font_size, Some(18.0));
    s.reset("terminal.font.size", WriteTarget::Auto).unwrap();
    assert!(fs::read_to_string(&f.0.terminal).unwrap().contains("Mono"));
    let text = fs::read_to_string(&f.0.ultra).unwrap();
    assert!(text.contains("# keep"));
    assert!(text.contains("# inline"));
    assert!(text.contains("Atlas"));
}
#[test]
fn oversized_configuration_preserves_last_good_and_refuses_write() {
    let f = Fixture::new();
    fs::write(&f.0.ultra, "[sidebar]\nwidth=280\n").unwrap();
    let mut s = ConfigStore::load(f.0.clone());
    let file = fs::OpenOptions::new().write(true).open(&f.0.ultra).unwrap();
    file.set_len(write::MAX_CONFIG_BYTES + 1).unwrap();
    s.reload();
    assert_eq!(s.settings().sidebar_width, 280);
    assert!(s
        .snapshot()
        .errors
        .iter()
        .any(|e| e.message.contains("1 MiB")));
    assert!(s
        .set("sidebar.width", serde_json::json!(300), WriteTarget::Auto)
        .is_err());
    for path in [&f.0.terminal, &f.0.keys, &f.0.machine_file] {
        let file = fs::File::create(path).unwrap();
        file.set_len(write::MAX_CONFIG_BYTES + 1).unwrap();
    }
    s.reload();
    assert_eq!(s.snapshot().errors.len(), 4);
}
#[test]
fn icon_overrides_reload_and_fall_back_without_blocking_settings() {
    let f = Fixture::new();
    fs::write(&f.0.ultra, "[sidebar]\nprovider_mark='off'\n[icons]\nbrand_color='off'\n[tray]\nicon='dark'\n[tools.claude]\nicon='mark.data'\nicon_tint='original'\nicon_fill=true\n[tools.custom]\ncommand='custom'\n[providers.anthropic]\nicon='mark.data'\n").unwrap();
    let mut store = ConfigStore::load(f.0.clone());
    assert!(store.settings().tool("claude").unwrap().icon.is_none());
    assert!(store.settings().provider_icons.is_empty());
    assert_eq!(store.snapshot().errors.len(), 1);
    assert_eq!(store.settings().sidebar_provider_mark, "off");
    assert_eq!(store.settings().brand_color, "off");
    assert_eq!(store.settings().tray_icon, "dark");
    assert_eq!(store.settings().tool("custom").unwrap().glyph, "");
    fs::write(
        f.0.dir.join("mark.data"),
        "<svg xmlns='http://www.w3.org/2000/svg'/>",
    )
    .unwrap();
    assert!(store.reload().settings);
    let icon = store
        .settings()
        .tool("claude")
        .unwrap()
        .icon
        .as_ref()
        .unwrap();
    assert_eq!(icon.tint, IconTint::Original);
    assert!(icon.fill);
    assert_eq!(store.settings().provider_icons["anthropic"], icon.url);
    assert!(store.snapshot().errors.is_empty());
    fs::write(f.0.dir.join("mark.data"), "not an image").unwrap();
    assert!(store.reload().settings);
    assert!(store.settings().tool("claude").unwrap().icon.is_none());
    assert!(store.settings().provider_icons.is_empty());
    assert_eq!(store.snapshot().errors.len(), 1);
    fs::write(
        &f.0.ultra,
        "[tools.custom]\ncommand='custom'\nglyph='XY'\nicon='mark.data'\n",
    )
    .unwrap();
    store.reload();
    assert_eq!(store.settings().tool("custom").unwrap().glyph, "XY");
    assert!(store.settings().tool("custom").unwrap().icon.is_none());
}

#[test]
fn icon_errors_open_the_key_source_instead_of_the_image() {
    let f = Fixture::new();
    fs::write(
        &f.0.ultra,
        "[tools.claude]\nicon='unused.svg'\n[providers.anthropic]\nicon='missing-provider.svg'\n",
    )
    .unwrap();
    fs::write(
        &f.0.machine_file,
        "# Machine override\n[tools.claude]\nicon='invalid-tool.svg'\n",
    )
    .unwrap();
    fs::write(f.0.dir.join("invalid-tool.svg"), "not an image").unwrap();
    let store = ConfigStore::load(f.0.clone());
    let errors = store.snapshot().errors;
    assert_eq!(errors.len(), 2);
    for (key, source, line, image) in [
        (
            "tools.claude.icon",
            &f.0.machine_file,
            3,
            "invalid-tool.svg",
        ),
        (
            "providers.anthropic.icon",
            &f.0.ultra,
            4,
            "missing-provider.svg",
        ),
    ] {
        let error = errors
            .iter()
            .find(|e| e.key.as_deref() == Some(key))
            .unwrap();
        assert_eq!(error.file, source.to_string_lossy());
        assert_eq!(error.line, Some(line));
        assert!(error
            .message
            .contains(f.0.dir.join(image).to_str().unwrap()));
        assert!(error.message.contains("using the built-in mark"));
        assert!(store
            .ensure_starter(std::path::Path::new(&error.file))
            .is_ok());
    }
    assert!(store.settings().tool("claude").unwrap().icon.is_none());
    assert!(store.settings().provider_icons.is_empty());
}

#[test]
fn tray_show_defaults_to_auto_reloads_and_rejects_unknown_values() {
    let f = Fixture::new();
    let mut store = ConfigStore::load(f.0.clone());
    assert_eq!(store.settings().tray_show, TrayShow::Auto);
    fs::write(&f.0.ultra, "[tray]\nicon='dark'\nshow='never'\n").unwrap();
    store.reload();
    assert_eq!(store.settings().tray_show, TrayShow::Never);
    assert_eq!(
        store.settings().tray_icon,
        "dark",
        "show and icon are separate keys"
    );
    fs::write(&f.0.ultra, "[tray]\nshow='always'\n").unwrap();
    store.reload();
    assert_eq!(store.settings().tray_show, TrayShow::Always);
    fs::write(&f.0.ultra, "[tray]\nshow='hidden'\n").unwrap();
    store.reload();
    assert_eq!(store.snapshot().errors.len(), 1);
    assert_eq!(
        store.settings().tray_show,
        TrayShow::Always,
        "an invalid value keeps the last good one"
    );
}

#[test]
fn removed_icon_mode_and_invalid_icon_choices_are_reported() {
    let f = Fixture::new();
    fs::write(&f.0.ultra, "[sidebar]\nicon='model'\nprovider_mark='tiny'\n[icons]\nbrand_color='always'\n[tray]\nicon='auto'\n[tools.omp]\nicon_tint='blue'\nicon_fill='yes'\naccent='mint'\n[tools.shell]\naccent='#ABCDEF'\n").unwrap();
    let store = ConfigStore::load(f.0.clone());
    assert_eq!(store.snapshot().errors.len(), 7);
    assert_eq!(store.settings().sidebar_provider_mark, "corner");
    assert_eq!(store.settings().brand_color, "all");
    assert_eq!(store.settings().tray_icon, "light");
    // An invalid accent keeps the built-in one; a valid one replaces it, normalized.
    assert_eq!(
        store.settings().tool("omp").unwrap().accent.as_deref(),
        Some("#7fdbca")
    );
    assert_eq!(
        store.settings().tool("shell").unwrap().accent.as_deref(),
        Some("#abcdef")
    );
    assert!(!store
        .snapshot()
        .values
        .iter()
        .any(|v| v.key == "sidebar.icon"));
}
#[test]
fn edits_create_missing_parents_in_the_parent_style_and_resets_never_create_them() {
    let f = Fixture::new();
    let key = |k: &str| k.split('.').map(String::from).collect::<Vec<_>>();
    let text = "font = { size = 15.0 } # font\n[sidebar]\nwidth = 248\n";
    fs::write(&f.0.ultra, text).unwrap();

    write::edit(&f.0.ultra, &key("launch.default_tool"), None).unwrap();
    assert_eq!(fs::read_to_string(&f.0.ultra).unwrap(), text);

    let family = toml::Value::String("Mono".into());
    write::edit(&f.0.ultra, &key("font.bold.family"), Some(&family)).unwrap();
    write::edit(&f.0.ultra, &key("launch.default_tool"), Some(&family)).unwrap();
    let doc = fs::read_to_string(&f.0.ultra)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert!(doc["font"]["bold"].is_inline_table(), "{doc}");
    assert_eq!(doc["font"]["bold"]["family"].as_str(), Some("Mono"));
    assert_eq!(doc["font"]["size"].as_float(), Some(15.0));
    assert!(doc["launch"].is_table(), "{doc}");
    assert_eq!(doc["launch"]["default_tool"].as_str(), Some("Mono"));
    assert!(doc.to_string().contains("# font"));
}
