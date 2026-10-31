use super::*;
use std::{io::Write, os::unix::fs::symlink};
use tempfile::TempDir;

#[test]
fn bounded_reads_status_and_successful_revisions() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("report.MD");
    fs::write(&path, "# Report\n").unwrap();
    let mut file = open(&path, dir.path(), true, 10).unwrap();
    assert_eq!(file.state.status, DocumentStatus::Ok);
    assert_eq!(file.state.revision, 1);
    assert!(file.state.follow);
    assert_eq!(file.state.dir, ".");
    assert_eq!(file.text, "# Report\n");
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(MAX_BYTES + 1)
        .unwrap();
    file.reload(20, false);
    assert_eq!(file.state.status, DocumentStatus::TooLarge);
    assert_eq!(file.state.revision, 1);
    assert!(file.text.is_empty());
    fs::write(&path, [0xff, 0xfe]).unwrap();
    file.reload(30, false);
    assert_eq!(file.state.status, DocumentStatus::Unreadable);
    assert!(file.state.error.is_some());
    fs::write(&path, "returned").unwrap();
    file.reload(40, false);
    assert_eq!(file.state.status, DocumentStatus::Ok);
    assert_eq!(file.state.revision, 2);
    assert_eq!(file.state.live_until_ms, file.state.modified_ms + 5000);
    assert!(file.state.error.is_none());
}

#[test]
fn assets_are_bounded_and_confined_even_through_symlinks() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("docs");
    fs::create_dir(&root).unwrap();
    let doc = root.join("a.md");
    fs::write(&doc, "hello").unwrap();
    let doc = doc.canonicalize().unwrap();
    // A real PNG header is enough for content-based media classification.
    let image = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01";
    fs::write(root.join("image.png"), image).unwrap();
    fs::write(dir.path().join("outside.png"), image).unwrap();
    symlink(dir.path().join("outside.png"), root.join("escape.png")).unwrap();
    assert_eq!(asset(&doc, "image.png").unwrap().mime, "image/png");
    assert!(asset(&doc, "../outside.png").is_err());
    assert!(asset(&doc, "escape.png").is_err());
    assert!(asset(&doc, dir.path().join("outside.png").to_str().unwrap()).is_err());
    let large = File::create(root.join("large.png")).unwrap();
    large.set_len(MAX_BYTES + 1).unwrap();
    assert!(asset(&doc, "large.png").unwrap_err().contains("8 MiB"));
}

#[test]
fn recent_scan_honors_ignore_hidden_bounds_and_ordering() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::create_dir(dir.path().join(".hidden")).unwrap();
    fs::write(dir.path().join(".gitignore"), "ignored.md\n").unwrap();
    fs::write(dir.path().join("ignored.md"), "ignored").unwrap();
    fs::write(dir.path().join(".hidden/no.md"), "hidden").unwrap();
    for n in 0..55 {
        let file = File::create(dir.path().join(format!("{n:02}.MARKDOWN"))).unwrap();
        file.set_times(
            fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(100 + n)),
        )
        .unwrap();
    }
    let list = scan(dir.path());
    assert_eq!(list.entries.len(), 50);
    assert_eq!(list.entries[0].name, "54.MARKDOWN");
    assert_eq!(list.entries.last().unwrap().name, "05.MARKDOWN");
    assert!(!list.truncated);
    assert!(scan_bounded(dir.path(), 3, Duration::from_secs(2)).truncated);
    assert!(scan_bounded(dir.path(), 20_000, Duration::ZERO).truncated);
}

#[test]
fn preview_uses_only_the_bounded_prefix_and_ignores_frontmatter_and_fences() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("report.md");
    let mut file = File::create(&path).unwrap();
    file.write_all(b"---\nname: report\n---\n# Report\n\nFirst paragraph\ncontinues.\n\n```md\n# Not a heading\n```\n## Detail\n").unwrap();
    file.write_all(&vec![b' '; 64 * 1024]).unwrap();
    file.write_all(b"\n# Outside prefix\n").unwrap();
    let result = preview(&path).unwrap();
    assert_eq!(result.h1.as_deref(), Some("Report"));
    assert_eq!(result.heading_count, 2);
    assert_eq!(
        result.first_paragraph.as_deref(),
        Some("First paragraph continues.")
    );
    assert!(result.size_bytes > 64 * 1024);
}

fn wait_event(
    manager: &mut Manager,
    rx: &mpsc::Receiver<Event>,
    expected: DocumentEvent,
) -> DocumentState {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let delay = manager
            .timeout(Instant::now())
            .min(Duration::from_millis(20));
        if let Ok(event) = rx.recv_timeout(delay) {
            manager.event(event, Instant::now());
        }
        let mut found = None;
        manager.due(Instant::now(), crate::workspace::now(), |id, state| {
            if id == "a" && state.last_event == expected {
                found = Some(state);
            }
        });
        if let Some(state) = found {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "watcher did not report {expected:?}"
        );
    }
}

#[test]
fn real_directory_watch_reports_update_replace_missing_and_return() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("report.md");
    let second = dir.path().join("other.md");
    fs::write(&path, "initial").unwrap();
    fs::write(&second, "other").unwrap();
    let (tx, rx) = mpsc::channel();
    let mut manager = Manager::new(move |event| {
        if let Ok(event) = event {
            let _ = tx.send(event);
        }
    })
    .unwrap();
    manager
        .watch("a".into(), open(&path, dir.path(), false, 1).unwrap())
        .unwrap();
    manager
        .watch("b".into(), open(&second, dir.path(), false, 1).unwrap())
        .unwrap();
    assert_eq!(manager.directories.len(), 1);
    wait_event(&mut manager, &rx, DocumentEvent::Opened);
    File::options()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b" appended")
        .unwrap();
    let update = wait_event(&mut manager, &rx, DocumentEvent::Updated);
    assert_eq!(update.status, DocumentStatus::Ok);
    assert!(update.live_until_ms > crate::workspace::now());
    assert!(manager.text("a").unwrap().text.ends_with("appended"));
    fs::write(&path, "short").unwrap();
    wait_event(&mut manager, &rx, DocumentEvent::Replaced);
    let replacement = dir.path().join("replacement");
    fs::write(&replacement, "replacement with another inode").unwrap();
    fs::rename(replacement, &path).unwrap();
    wait_event(&mut manager, &rx, DocumentEvent::Replaced);
    fs::remove_file(&path).unwrap();
    let removed = Instant::now();
    let missing = wait_event(&mut manager, &rx, DocumentEvent::Missing);
    assert!(removed.elapsed() >= MISSING_GRACE);
    assert_eq!(missing.status, DocumentStatus::Missing);
    assert!(manager.text("a").is_err());
    fs::write(&path, "back").unwrap();
    let returned = wait_event(&mut manager, &rx, DocumentEvent::Returned);
    assert_eq!(returned.status, DocumentStatus::Ok);
    assert_eq!(manager.text("a").unwrap().text, "back");
    manager.forget("a");
    assert_eq!(manager.directories.len(), 1);
    manager.forget("b");
    assert!(manager.directories.is_empty());
}

#[test]
fn missing_grace_keeps_atomic_saves_ok_with_injected_clock() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("a.md");
    fs::write(&path, "first").unwrap();
    let mut manager = Manager::new(|_| {}).unwrap();
    manager
        .watch("a".into(), open(&path, dir.path(), false, 1).unwrap())
        .unwrap();
    let baseline = Instant::now() + COALESCE;
    manager.due(baseline, 1, |_, _| {});
    fs::remove_file(&path).unwrap();
    manager.event(
        Event::new(EventKind::Remove(notify::event::RemoveKind::File)).add_path(path.clone()),
        baseline,
    );
    let at = baseline + COALESCE;
    manager.due(at, 100, |_, _| panic!("missing flashed before its grace"));
    manager.due(at + Duration::from_millis(499), 599, |_, _| {
        panic!("missing flashed before 500 ms")
    });
    fs::write(&path, "second").unwrap();
    let mut states = Vec::new();
    manager.due(at + MISSING_GRACE, 600, |_, state| states.push(state));
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].status, DocumentStatus::Ok);
    assert_eq!(states[0].last_event, DocumentEvent::Replaced);
}

#[test]
fn external_links_cannot_dispatch_launchers_or_navigate_the_webview() {
    for url in [
        "https://example.com/path",
        "http://localhost/path",
        "mailto:a@example.com",
    ] {
        assert!(external_link(url).is_ok());
    }
    for url in [
        "file:///tmp/report.md",
        "javascript:alert(1)",
        "data:text/html,test",
        "my-launcher:run",
        "/relative",
    ] {
        assert!(external_link(url).is_err(), "{url}");
    }
}

#[test]
fn scan_cache_refreshes_for_each_palette_presentation() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("first.md"), "# First\n").unwrap();
    assert_eq!(project_documents(dir.path(), 1).entries.len(), 1);
    fs::write(dir.path().join("second.md"), "# Second\n").unwrap();
    assert_eq!(project_documents(dir.path(), 1).entries.len(), 1);
    assert_eq!(project_documents(dir.path(), 2).entries.len(), 2);
}

#[test]
fn directory_watch_recovers_when_the_parent_directory_returns() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path().join("docs");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("a.md");
    fs::write(&path, "# Before\n").unwrap();
    let (tx, rx) = mpsc::channel();
    let mut manager = Manager::new(move |event| {
        if let Ok(event) = event {
            let _ = tx.send(event);
        }
    })
    .unwrap();
    manager
        .watch("a".into(), open(&path, &parent, false, 1).unwrap())
        .unwrap();
    wait_event(&mut manager, &rx, DocumentEvent::Opened);
    fs::remove_dir_all(&parent).unwrap();
    wait_event(&mut manager, &rx, DocumentEvent::Missing);
    fs::create_dir(&parent).unwrap();
    fs::write(&path, "# Returned\n").unwrap();
    wait_event(&mut manager, &rx, DocumentEvent::Returned);
    fs::write(&path, "# Updated after return\n").unwrap();
    wait_event(&mut manager, &rx, DocumentEvent::Updated);
}

#[test]
fn assets_cannot_expand_scope_when_the_document_directory_is_replaced_by_a_symlink() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("docs");
    let outside = dir.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    let doc = root.join("a.md");
    fs::write(&doc, "# Document\n").unwrap();
    let doc = doc.canonicalize().unwrap();
    fs::write(outside.join("image.png"), b"\x89PNG\r\n\x1a\n").unwrap();
    fs::rename(&root, dir.path().join("old-docs")).unwrap();
    symlink(&outside, &root).unwrap();
    assert!(asset(&doc, "image.png").unwrap_err().contains("escapes"));
}
