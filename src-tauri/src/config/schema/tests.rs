use super::*;
use serde_json::json;

fn check(path: &[&str], value: &str) -> Result<(), String> {
    let table: toml::Table = format!("v = {value}").parse().unwrap();
    let path: Vec<String> = path.iter().map(|s| (*s).into()).collect();
    validate(&path, &table["v"])
}

#[test]
fn write_target_wire_shapes_round_trip_and_reject_others() {
    for (target, wire) in [
        (WriteTarget::Auto, json!("auto")),
        (WriteTarget::Machine, json!("machine")),
        (WriteTarget::Project("/p".into()), json!({"project": "/p"})),
    ] {
        assert_eq!(serde_json::to_value(&target).unwrap(), wire);
        let back: WriteTarget = serde_json::from_value(wire).unwrap();
        assert_eq!(format!("{back:?}"), format!("{target:?}"));
    }
    for wire in [
        json!("project"),
        json!(5),
        json!({"project": 1}),
        json!({"folder": "/p"}),
        json!({"project": "/p", "extra": true}),
    ] {
        assert!(
            serde_json::from_value::<WriteTarget>(wire.clone()).is_err(),
            "{wire}"
        );
    }
}

#[test]
fn project_name_prefers_override_then_folder_name() {
    let mut s = Settings::default();
    s.projects.insert(
        "/work/atlas".into(),
        ProjectOverride {
            name: Some("Atlas".into()),
            default_tool: None,
        },
    );
    s.projects
        .insert("/work/plain".into(), ProjectOverride::default());
    assert_eq!(s.project_name(Path::new("/work/atlas")), "Atlas");
    assert_eq!(s.project_name(Path::new("/work/plain")), "plain");
    assert_eq!(s.project_name(Path::new("/work/other")), "other");
    assert_eq!(s.project_name(Path::new("/")), "/");
}

#[test]
fn builtins_match_the_documented_tool_table() {
    let tools = builtins();
    let row = |id: &str| {
        let t = tools.iter().find(|t| t.id == id).unwrap();
        (
            t.command.as_deref(),
            t.resume.clone(),
            t.sessions,
            t.status,
            t.kind,
        )
    };
    let resume = |a: &str, b: &str| Some(vec![a.to_owned(), b.to_owned()]);
    use {StatusSource::Process, StoreKind::*, ToolKind::*};
    assert_eq!(
        row("omp"),
        (
            Some("omp"),
            resume("--resume", "{path}"),
            Some(Omp),
            StatusSource::Omp,
            Agent
        )
    );
    assert_eq!(
        row("claude"),
        (
            Some("claude"),
            resume("--resume", "{id}"),
            Some(Claude),
            Process,
            Agent
        )
    );
    assert_eq!(
        row("codex"),
        (
            Some("codex"),
            resume("resume", "{id}"),
            Some(Codex),
            Process,
            Agent
        )
    );
    assert_eq!(
        row("pi"),
        (
            Some("pi"),
            resume("--session", "{path}"),
            Some(Pi),
            Process,
            Agent
        )
    );
    assert_eq!(row("shell"), (None, None, None, Process, Terminal));
    assert_eq!(row("neovim"), (Some("nvim"), None, None, Process, Terminal));
    assert!(tools.iter().all(|t| t.env.is_empty()));
}

#[test]
fn every_default_is_valid_and_every_ui_key_has_one() {
    let defaults = defaults();
    for (key, value) in &defaults {
        assert_eq!(validate(key, value), Ok(()), "{}", key_name(key));
    }
    for key in UI_KEYS {
        let path: Vec<String> = key.split('.').map(Into::into).collect();
        assert!(defaults.contains_key(&path), "{key}");
    }
    let s = Settings::default();
    let key = |k: &str| -> Vec<String> { k.split('.').map(Into::into).collect() };
    assert_eq!(
        defaults[&key("sidebar.width")].as_integer(),
        Some(s.sidebar_width.into())
    );
    assert_eq!(
        defaults[&key("launch.default_tool")].as_str(),
        Some(s.default_tool.as_str())
    );
}

#[test]
fn key_name_quotes_only_components_that_need_it() {
    let name = |parts: &[&str]| key_name(&parts.iter().map(|s| (*s).into()).collect::<Vec<_>>());
    assert_eq!(name(&["sidebar", "width"]), "sidebar.width");
    assert_eq!(
        name(&["projects", "/work/a.b", "default_tool"]),
        "projects.\"/work/a.b\".default_tool"
    );
    assert_eq!(name(&["tools", "a\"b", "label"]), "tools.\"a\\\"b\".label");
    assert_eq!(name(&["tools", "a\\b", "label"]), "tools.\"a\\\\b\".label");
}

#[test]
fn numeric_settings_accept_inclusive_ranges_only() {
    for (path, ok, bad, expected) in [
        (
            &["sidebar", "width"][..],
            ["240", "360"],
            ["239", "361"],
            "expected integer 240–360",
        ),
        (
            &["sidebar", "auto_collapse_below"],
            ["0", "10000"],
            ["-1", "10001"],
            "expected integer 0–10000",
        ),
        (
            &["resume", "open_elsewhere_hint_seconds"],
            ["0", "86400"],
            ["-1", "86401"],
            "expected integer 0–86400",
        ),
        (
            &["terminal", "font", "size"],
            ["4", "96.0"],
            ["3.9", "97"],
            "expected number 4–96",
        ),
    ] {
        for value in ok {
            assert_eq!(check(path, value), Ok(()), "{path:?} = {value}");
        }
        for value in bad {
            assert_eq!(
                check(path, value).unwrap_err(),
                expected,
                "{path:?} = {value}"
            );
        }
    }
    assert!(check(&["sidebar", "width"], "300.0").is_err());
    assert!(check(&["terminal", "font", "size"], "nan").is_err());
    assert!(check(&["terminal", "font", "size"], "inf").is_err());
    assert!(check(&["terminal", "font", "size"], "'12'").is_err());
}

#[test]
fn choice_text_and_boolean_settings_validate_their_type() {
    for (path, ok, bad, expected) in [
        (
            &["launch", "default_tool"][..],
            "'omp'",
            "' '",
            "expected nonempty tool id",
        ),
        (
            &["sidebar", "density"],
            "'compact'",
            "'dense'",
            "expected comfortable | compact",
        ),
        (
            &["sidebar", "provider_mark"],
            "'off'",
            "'on'",
            "expected corner | off",
        ),
        (
            &["icons", "brand_color"],
            "'pickers'",
            "'some'",
            "expected all | pickers | launch | off",
        ),
        (
            &["tray", "icon"],
            "'dark'",
            "'auto'",
            "expected light | dark",
        ),
        (
            &["tray", "show"],
            "'never'",
            "'dark'",
            "expected auto | always | never",
        ),
        (
            &["layout", "context_bar"],
            "'always'",
            "'sometimes'",
            "expected auto | always | never",
        ),
        (
            &["notify", "needs_you"],
            "false",
            "'false'",
            "expected boolean",
        ),
        (
            &["notify", "turn_finished"],
            "true",
            "1",
            "expected boolean",
        ),
        (
            &["terminals", "remove_on_clean_exit"],
            "true",
            "'yes'",
            "expected boolean",
        ),
        (
            &["terminals", "confirm_close_with_foreground_job"],
            "false",
            "0",
            "expected boolean",
        ),
        (
            &["confirm", "close_unmonitored_agents"],
            "true",
            "[]",
            "expected boolean",
        ),
        (
            &["projects", "pinned"],
            "['/a', '/b']",
            "['/a', 'b']",
            "expected array of absolute paths",
        ),
        (
            &["projects", "roots"],
            "[]",
            "[1]",
            "expected array of absolute paths",
        ),
        (
            &["projects", "/p", "name"],
            "'Atlas'",
            "''",
            "expected nonempty string",
        ),
        (
            &["projects", "/p", "default_tool"],
            "'pi'",
            "7",
            "expected nonempty string",
        ),
        (
            &["providers", "openai", "icon"],
            "'icons/o.svg'",
            "''",
            "expected nonempty relative icon path",
        ),
    ] {
        assert_eq!(check(path, ok), Ok(()), "{path:?} = {ok}");
        assert_eq!(check(path, bad).unwrap_err(), expected, "{path:?} = {bad}");
    }
    assert!(check(&["projects", "roots"], "'/a'").is_err());
}

#[test]
fn tool_fields_validate_shape_and_environment_safety() {
    for (field, ok, bad, expected) in [
        ("label", "'Aider'", "''", "expected nonempty string"),
        ("glyph", "'ai'", "1", "expected nonempty string"),
        ("command", "'aider'", "' '", "expected nonempty string"),
        (
            "icon",
            "'icons/a.svg'",
            "''",
            "expected nonempty relative icon path",
        ),
        (
            "icon_tint",
            "'original'",
            "'blue'",
            "expected mono | original",
        ),
        ("icon_fill", "true", "'yes'", "expected boolean"),
        ("accent", "'#A1b2C3'", "'#a1b2c'", "expected #rrggbb colour"),
        ("args", "['--x']", "'--x'", "expected string array"),
        ("resume", "['{id}']", "[1]", "expected string array"),
        ("kind", "'terminal'", "'shell'", "expected agent | terminal"),
        (
            "sessions",
            "'codex'",
            "'cursor'",
            "expected omp | claude | codex | pi",
        ),
        ("status", "'process'", "'title'", "expected omp | process"),
    ] {
        assert_eq!(check(&["tools", "x", field], ok), Ok(()), "{field} = {ok}");
        assert_eq!(
            check(&["tools", "x", field], bad).unwrap_err(),
            expected,
            "{field} = {bad}"
        );
    }
    for accent in ["'a1b2c3d'", "'#a1b2cg'", "'#a1b2c3d'"] {
        assert!(
            check(&["tools", "x", "accent"], accent).is_err(),
            "{accent}"
        );
    }
    assert_eq!(check(&["tools", "x", "env", "FOO"], "'bar'"), Ok(()));
    for (name, value) in [
        ("A=B", "'v'"),
        ("A\0", "'v'"),
        ("FOO", "\"a\\u0000b\""),
        ("FOO", "1"),
    ] {
        assert_eq!(
            check(&["tools", "x", "env", name], value).unwrap_err(),
            "expected environment string",
            "{name:?} = {value}"
        );
    }
}

#[test]
fn unknown_or_misplaced_keys_are_rejected() {
    for path in [
        &["foo", "bar"][..],
        &["sidebar", "icon"],
        &["projects", "relative", "name"],
        &["projects", "/p", "roots"],
        &["providers", "openai", "label"],
        &["tools", "x", "env"],
        &["tools", "x", "label", "nested"],
        &["tools", "x", "env", "A", "B"],
        &["tools", "x", "colour"],
    ] {
        assert_eq!(check(path, "'x'").unwrap_err(), "unknown key", "{path:?}");
    }
}
