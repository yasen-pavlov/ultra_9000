use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use super::{
    executable_shell, path_check, posix_shell, prepare, probe_login_path, tool_launch_via,
    EXEC_SCRIPT,
};
use super::{find_executable, validate_directory};
use crate::config::{Settings, StatusSource};
use std::path::Path;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("ultra9000-launch-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn pairs(env: &[(String, String)]) -> Vec<(&str, &str)> {
    env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
}

#[test]
fn directory_must_be_an_existing_absolute_directory() {
    let dir = scratch("dir");
    let file = dir.join("file");
    fs::write(&file, b"").unwrap();

    assert!(validate_directory("relative/path").is_err());
    assert!(validate_directory("").is_err());
    assert!(validate_directory(dir.join("missing").to_str().unwrap()).is_err());
    assert!(validate_directory(file.to_str().unwrap()).is_err());

    // Trailing separators are normalized; symlinks are not resolved.
    let with_slash = format!("{}/", dir.display());
    assert_eq!(validate_directory(&with_slash).unwrap(), dir);

    // Whitespace is part of a directory name, never trimmed away.
    let spaced = dir.join(" spaced ");
    fs::create_dir_all(&spaced).unwrap();
    assert_eq!(
        validate_directory(spaced.to_str().unwrap()).unwrap(),
        spaced
    );
    assert!(validate_directory("   ").is_err());

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn executable_lookup_follows_path_order_and_skips_inaccessible_candidates() {
    let dir = scratch("path");
    let dirs = ["plain", "others-only", "usable"].map(|name| dir.join(name));
    for entry in &dirs {
        fs::create_dir_all(entry).unwrap();
    }

    // No execute bit at all.
    let plain = dirs[0].join("tool");
    fs::write(&plain, b"#!/bin/sh\n").unwrap();
    fs::set_permissions(&plain, fs::Permissions::from_mode(0o644)).unwrap();
    // Owned by the current user, executable only by others: not runnable by us.
    let others_only = dirs[1].join("tool");
    fs::write(&others_only, b"#!/bin/sh\n").unwrap();
    fs::set_permissions(&others_only, fs::Permissions::from_mode(0o001)).unwrap();
    let usable = dirs[2].join("tool");
    fs::write(&usable, b"#!/bin/sh\n").unwrap();
    fs::set_permissions(&usable, fs::Permissions::from_mode(0o755)).unwrap();

    let path = format!(
        "relative:{}:{}:{}",
        dirs[0].display(),
        dirs[1].display(),
        dirs[2].display()
    );
    assert_eq!(find_executable("tool", &path), Some(usable));
    assert_eq!(find_executable("missing", &path), None);

    fs::set_permissions(&others_only, fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}
#[test]
fn argv_templates_remain_positional_data() {
    let template = vec![
        "--resume".into(),
        "{path}".into(),
        "--id={id}".into(),
        "{cwd}".into(),
    ];
    assert_eq!(
        super::substitute(
            &template,
            "{path};$(touch nope)",
            "/tmp/a \"quoted\" file",
            "/tmp/{id}"
        ),
        vec![
            "--resume",
            "/tmp/a \"quoted\" file",
            "--id={path};$(touch nope)",
            "/tmp/{id}"
        ]
    );
    assert_eq!(super::EXEC_SCRIPT, "exec \"$0\" \"$@\"");
}
#[test]
fn path_check_suggests_closest_enterable_sibling() {
    let dir = scratch("suggestion");
    fs::create_dir(dir.join("atlas")).unwrap();
    fs::create_dir(dir.join("meridian")).unwrap();
    fs::write(dir.join("atla"), "").unwrap();
    let result = super::path_check(dir.join("atls").to_str().unwrap());
    assert!(result.error.is_some());
    assert!(result.normalized.is_none());
    assert_eq!(
        result.suggestion,
        Some(dir.join("atlas").to_string_lossy().into_owned())
    );
    assert!(super::path_check(dir.join("atlas").to_str().unwrap())
        .error
        .is_none());
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn prepared_environment_adds_omp_status_protocol_and_respects_tool_env() {
    let dir = scratch("prepare");
    let cwd = dir.to_str().unwrap();
    let mut tool = Settings::default().tool("shell").unwrap().clone();
    assert_eq!(tool.command, None);
    tool.env = vec![
        ("FOO".into(), "bar".into()),
        ("WARP_CLI_AGENT_PROTOCOL_VERSION".into(), "9".into()),
    ];

    let plain = prepare(&tool, &format!("{cwd}/"), Vec::new()).unwrap();
    assert!(plain.command.is_none(), "no argv starts the login shell");
    assert_eq!(plain.working_directory, dir);
    assert_eq!(pairs(&plain.env), pairs(&tool.env));

    tool.status = StatusSource::Omp;
    let omp = prepare(&tool, cwd, vec!["+3".into(), "notes.md".into()]).unwrap();
    let (_, args) = omp.command.unwrap();
    assert_eq!(args, ["+3", "notes.md"]);
    assert_eq!(
        pairs(&omp.env),
        [
            ("FOO", "bar"),
            ("WARP_CLI_AGENT_PROTOCOL_VERSION", "1"),
            ("PI_NOTIFICATIONS", "off"),
        ]
    );

    tool.env.push(("PI_NOTIFICATIONS".into(), "on".into()));
    let own = prepare(&tool, cwd, Vec::new()).unwrap();
    assert_eq!(
        pairs(&own.env),
        [
            ("FOO", "bar"),
            ("PI_NOTIFICATIONS", "on"),
            ("WARP_CLI_AGENT_PROTOCOL_VERSION", "1"),
        ]
    );

    let missing = dir.join("missing");
    assert_eq!(
        prepare(&tool, missing.to_str().unwrap(), Vec::new()).unwrap_err(),
        format!("{} does not exist.", missing.display())
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn tools_start_through_the_login_shell_with_argv_as_data() {
    let dir = scratch("tool-launch");
    let bin = dir.join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = script(&bin, "agent", "for arg in \"$@\"; do echo \"[$arg]\"; done");
    let path = format!("/u9k-nowhere:{}", bin.display());
    let extra = vec!["--resume".to_owned(), "a b".into(), "$(touch pwned)".into()];

    let launch = tool_launch_via(
        "/bin/sh".into(),
        &path,
        "agent",
        "Agent",
        dir.clone(),
        extra,
    )
    .unwrap();
    let exe = tool.to_string_lossy().into_owned();
    assert_eq!(launch.executable.as_deref(), Some(exe.as_str()));
    assert_eq!(launch.working_directory, dir);
    let (shell, args) = launch.command.unwrap();
    assert_eq!(shell, "/bin/sh");
    assert_eq!(
        args,
        [
            "-l",
            "-i",
            "-c",
            EXEC_SCRIPT,
            &exe,
            "--resume",
            "a b",
            "$(touch pwned)"
        ]
    );
    // The exec script hands the remaining argv to the tool verbatim.
    let output = std::process::Command::new(&shell)
        .args(&args[2..])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "[--resume]\n[a b]\n[$(touch pwned)]\n"
    );
    assert!(!dir.join("pwned").exists());

    assert_eq!(
        tool_launch_via(
            "/bin/sh".into(),
            &path,
            "claude",
            "Claude Code",
            dir.clone(),
            vec![]
        )
        .unwrap_err(),
        "Claude Code (claude) was not found on your login shell's PATH."
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn only_posix_login_shells_can_start_tools() {
    for shell in [
        "/bin/sh",
        "/usr/bin/bash",
        "/bin/zsh",
        "/bin/ksh",
        "/bin/dash",
    ] {
        assert_eq!(posix_shell(shell.into(), "OMP").as_deref(), Ok(shell));
    }
    assert_eq!(
        posix_shell("/usr/bin/fish".into(), "OMP").unwrap_err(),
        "OMP sessions start through your login shell, but \u{201c}/usr/bin/fish\u{201d} is \
         not a supported POSIX shell (sh, bash, zsh, ksh, dash)."
    );
    assert!(posix_shell(String::new(), "OMP").is_err());
}

#[test]
fn shell_items_require_an_executable_login_shell_file() {
    let dir = scratch("login-shell");
    let shell = script(&dir, "myshell", "exit 0");
    let shell = shell.to_string_lossy().into_owned();
    assert_eq!(executable_shell(shell.clone()), Ok(shell.clone()));
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o644)).unwrap();
    for candidate in [shell, dir.to_string_lossy().into(), String::new()] {
        assert_eq!(
            executable_shell(candidate).unwrap_err(),
            "login shell is not executable"
        );
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn login_path_probe_reads_the_marker_line_of_an_interactive_login_shell() {
    let dir = scratch("probe");
    // Behaves like `sh -l -i -c SCRIPT` for the fixed probe script only.
    let shell = script(
        &dir,
        "fake-login-shell",
        "[ \"$1 $2 $3\" = '-l -i -c' ] || exit 3\necho 'motd from a profile'\n\
         PATH=/probe/bin:/usr/bin\neval \"$4\"",
    );
    assert_eq!(
        probe_login_path(shell.to_str().unwrap()).unwrap(),
        "/probe/bin:/usr/bin"
    );

    let silent = script(&dir, "silent", "exit 0");
    let silent = silent.to_str().unwrap();
    assert_eq!(
        probe_login_path(silent).unwrap_err(),
        format!("Unable to read PATH from your login shell ({silent}): it printed no PATH")
    );
    let empty = script(&dir, "empty", "printf '__ULTRA9000_LOGIN_PATH__=\\n'");
    assert!(probe_login_path(empty.to_str().unwrap())
        .unwrap_err()
        .ends_with("it printed an empty PATH"));
    let missing = dir.join("missing-shell");
    assert!(probe_login_path(missing.to_str().unwrap())
        .unwrap_err()
        .starts_with(&format!(
            "Unable to read PATH from your login shell ({}): ",
            missing.display()
        )));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_probe_shell_that_lingers_after_printing_is_killed_not_awaited() {
    let dir = scratch("probe-linger");
    let shell = script(
        &dir,
        "lingering",
        "PATH=/linger/bin\neval \"$4\"\nexec /bin/sleep 60",
    );
    let started = std::time::Instant::now();
    assert_eq!(
        probe_login_path(shell.to_str().unwrap()).unwrap(),
        "/linger/bin"
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed >= super::PROBE_EXIT_GRACE && elapsed < std::time::Duration::from_secs(20),
        "{elapsed:?}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn directory_errors_name_the_reason() {
    let dir = scratch("dir-errors");
    if let Some(home) = alacritty_embed::embed::home_directory() {
        let home = home.components().collect::<PathBuf>();
        assert_eq!(validate_directory("~").unwrap(), home);
        assert_eq!(validate_directory("~/").unwrap(), home);
    }
    assert_eq!(
        validate_directory("~other/x").unwrap_err(),
        "\u{201c}~other/x\u{201d} is not an absolute path."
    );

    let file = dir.join("file");
    fs::write(&file, "").unwrap();
    let below_file = file.join("child");
    assert!(validate_directory(below_file.to_str().unwrap())
        .unwrap_err()
        .starts_with(&format!("{} cannot be used: ", below_file.display())));

    // access(X_OK) succeeds for root, so a 0o600 directory is only locked for others.
    if unsafe { libc::geteuid() } != 0 {
        let locked = dir.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            validate_directory(locked.to_str().unwrap()).unwrap_err(),
            format!(
                "{} is not accessible (permission denied).",
                locked.display()
            )
        );
        let hidden = locked.join("inner");
        assert_eq!(
            validate_directory(hidden.to_str().unwrap()).unwrap_err(),
            format!(
                "{} is not accessible (permission denied).",
                hidden.display()
            )
        );
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let relative = path_check("relative/dir");
    assert_eq!(
        relative.error.as_deref(),
        Some("\u{201c}relative/dir\u{201d} is not an absolute path.")
    );
    assert_eq!(relative.suggestion, None);
    fs::remove_dir_all(dir).unwrap();
}
