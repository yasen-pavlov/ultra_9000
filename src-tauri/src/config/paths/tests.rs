use super::*;
use std::{collections::HashMap, path::Path};

fn resolve(vars: &[(&str, &str)]) -> Result<ConfigPaths, String> {
    let vars: HashMap<_, _> = vars.iter().copied().collect();
    ConfigPaths::resolve_with(|name| vars.get(name).map(OsString::from), || "host".into())
}

#[test]
fn defaults_follow_home_with_xdg_layout() {
    let p = resolve(&[("HOME", "/home/u")]).unwrap();
    let dir = Path::new("/home/u/.config/ultra-9000");
    assert_eq!(p.dir, dir);
    assert_eq!(p.ultra, dir.join("ultra.toml"));
    assert_eq!(p.terminal, dir.join("terminal.toml"));
    assert_eq!(p.keys, dir.join("keys.toml"));
    assert_eq!(p.machine, "host");
    assert_eq!(p.machine_file, dir.join("machines/host.toml"));
    assert_eq!(
        p.state_file,
        Path::new("/home/u/.local/state/ultra-9000/state.json")
    );
}

#[test]
fn xdg_overrides_apply_and_empty_values_mean_unset() {
    let p = resolve(&[
        ("HOME", "/home/u"),
        ("XDG_CONFIG_HOME", "/cfg"),
        ("XDG_STATE_HOME", "/st"),
        ("ULTRA_9000_MACHINE", "laptop"),
    ])
    .unwrap();
    assert_eq!(p.ultra, Path::new("/cfg/ultra-9000/ultra.toml"));
    assert_eq!(p.state_file, Path::new("/st/ultra-9000/state.json"));
    assert_eq!(
        p.machine_file,
        Path::new("/cfg/ultra-9000/machines/laptop.toml")
    );
    let p = resolve(&[
        ("HOME", "/home/u"),
        ("XDG_CONFIG_HOME", ""),
        ("XDG_STATE_HOME", ""),
        ("ULTRA_9000_MACHINE", ""),
    ])
    .unwrap();
    assert_eq!(p.dir, Path::new("/home/u/.config/ultra-9000"));
    assert_eq!(
        p.state_file,
        Path::new("/home/u/.local/state/ultra-9000/state.json")
    );
    assert_eq!(p.machine, "host");
}

#[test]
fn missing_home_and_relative_paths_are_rejected() {
    for vars in [&[][..], &[("HOME", "")]] {
        assert_eq!(resolve(vars).unwrap_err(), "HOME is unavailable");
    }
    // Each resolved directory must be absolute, whichever variable supplied it.
    for vars in [
        &[("HOME", "relative")][..],
        &[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "cfg")],
        &[("HOME", "/home/u"), ("XDG_STATE_HOME", "st")],
        &[("HOME", "relative"), ("XDG_CONFIG_HOME", "/cfg")],
    ] {
        assert_eq!(
            resolve(vars).unwrap_err(),
            "XDG paths must be absolute",
            "{vars:?}"
        );
    }
    assert!(resolve(&[
        ("HOME", "relative"),
        ("XDG_CONFIG_HOME", "/cfg"),
        ("XDG_STATE_HOME", "/st")
    ])
    .is_ok());
}

#[test]
fn machine_name_must_be_one_filename_component() {
    for machine in [".", "..", "a/b", "a\\b", "a\0b"] {
        assert_eq!(
            resolve(&[("HOME", "/home/u"), ("ULTRA_9000_MACHINE", machine)]).unwrap_err(),
            "machine name must be one filename component",
            "{machine:?}"
        );
    }
    assert_eq!(
        resolve(&[("HOME", "/home/u"), ("ULTRA_9000_MACHINE", "my.host")])
            .unwrap()
            .machine,
        "my.host"
    );
    let invalid = ConfigPaths::resolve_with(
        |name| match name {
            "HOME" => Some("/home/u".into()),
            "ULTRA_9000_MACHINE" => {
                use std::os::unix::ffi::OsStringExt;
                Some(OsString::from_vec(vec![0xff, b'x']))
            }
            _ => None,
        },
        || "fallback".into(),
    )
    .unwrap();
    assert_eq!(invalid.machine, "fallback");
    // The hostname fallback is validated like the variable.
    assert!(ConfigPaths::resolve_with(
        |name| (name == "HOME").then(|| "/home/u".into()),
        || "..".into()
    )
    .is_err());
}

#[test]
fn hostname_is_the_short_name() {
    let name = hostname();
    assert!(!name.contains(['.', '\0']), "{name:?}");
}
