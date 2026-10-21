// embed: Host file-link routing regressions; no native view or child process required.

use super::is_host_file_hint;
use crate::config::ui_config::Program;

fn default_opener() -> &'static str {
    if cfg!(target_os = "macos") { "open" } else { "xdg-open" }
}

#[test]
fn default_opener_recognizes_file_uri_schemes_and_all_extensions() {
    let commands = [
        Program::Just(default_opener().to_owned()),
        Program::WithArgs { program: default_opener().to_owned(), args: Vec::new() },
    ];
    for command in commands {
        for uri in [
            "file:///project/src/main.rs",
            "FILE:///project/a%20file.PNG#L12",
            "FiLe:///project/extensionless?line=3&column=2",
            "file:///project/%E2%98%83.txt",
        ] {
            assert!(is_host_file_hint(&command, uri));
        }
    }
}

#[test]
fn nonfile_links_remain_external_opener_actions() {
    let command = Program::Just(default_opener().to_owned());
    for text in [
        "https://example.com/file:///path",
        "http://example.com",
        "mailto:user@example.com",
        "ssh://localhost/path",
        "filed:///path",
        "/project/file.txt",
        "☃file:///path",
        "",
    ] {
        assert!(!is_host_file_hint(&command, text));
    }
}

#[test]
fn custom_commands_and_opener_arguments_keep_file_links_external() {
    let other_platform_opener = if cfg!(target_os = "macos") { "xdg-open" } else { "open" };
    let commands = [
        Program::Just("nvim".to_owned()),
        Program::Just("/usr/bin/open".to_owned()),
        Program::Just("/usr/bin/xdg-open".to_owned()),
        Program::Just(other_platform_opener.to_owned()),
        Program::WithArgs {
            program: default_opener().to_owned(),
            args: vec!["--".to_owned()],
        },
    ];
    for command in commands {
        assert!(!is_host_file_hint(&command, "file:///project/file.txt"));
    }
}

#[test]
fn bell_overflow_never_discards_or_reorders_clicked_file_links() {
    use std::collections::VecDeque;
    use crate::window_context::{push_host_event, PendingHostEvent};

    let mut events = VecDeque::new();
    for file in ["first", "second"] {
        push_host_event(&mut events, PendingHostEvent::OpenFile(file.to_owned()));
        for _ in 0..200 {
            push_host_event(&mut events, PendingHostEvent::Bell);
        }
    }
    let files: Vec<_> = events.into_iter().filter_map(|event| match event {
        PendingHostEvent::OpenFile(uri) => Some(uri),
        PendingHostEvent::Bell => None,
    }).collect();
    assert_eq!(files, ["first", "second"]);
}
