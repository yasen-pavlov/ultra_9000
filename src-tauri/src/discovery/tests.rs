#[test]
fn explicit_resume_accepts_hidden_stub_and_revalidates_identity() {
    let home = std::env::temp_dir().join(format!("u9k-bound-{}", std::process::id()));
    let bucket = home.join(".omp/agent/sessions/bucket");
    let cwd = home.join("project");
    fs::create_dir_all(&bucket).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    let path = bucket.join("date_id.jsonl");
    fs::write(
        &path,
        format!(
            "{{\"type\":\"session\",\"id\":\"id\",\"cwd\":{}}}\n",
            serde_json::to_string(&cwd).unwrap()
        ),
    )
    .unwrap();
    let tools = crate::config::builtins();
    let tool = tools.iter().find(|t| t.id == "omp").unwrap();
    let mut index = Index::new(home.clone());
    index.refresh(&tools, &[]);
    assert!(index.conversations().is_empty());
    let bound = read_bound(tool, "id", &path, &home, &[]).unwrap();
    assert_eq!(bound.cwd, cwd);
    assert!(read_bound(tool, "different", &path, &home, &[]).is_err());
    assert!(read_bound(tool, "id", &path, &home, &[home.join("other")]).is_err());
    fs::remove_dir_all(home).unwrap();
}
#[test]
fn symlink_roots_and_restored_cwd_refresh_without_store_changes() {
    use std::os::unix::fs::symlink;
    // Resolved like normalize_roots resolves paths (macOS: /var -> /private/var).
    let home = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("u9k-roots-{}", std::process::id()));
    let bucket = home.join(".omp/agent/sessions/bucket");
    let root = home.join("work");
    let link = home.join("work-link");
    let cwd = root.join("project");
    fs::create_dir_all(&bucket).unwrap();
    fs::create_dir_all(&root).unwrap();
    symlink(&root, &link).unwrap();
    let path = bucket.join("date_id.jsonl");
    fs::write(
        &path,
        format!(
            "{{\"type\":\"session\",\"id\":\"id\",\"title\":\"Saved\",\"cwd\":{}}}\n",
            serde_json::to_string(&cwd).unwrap()
        ),
    )
    .unwrap();
    let roots = [link.clone()];
    assert_eq!(normalize_roots(&roots), vec![root.clone()]);
    let mut index = Index::new(home.clone());
    let tools = crate::config::builtins();
    assert!(index.refresh(&tools, &roots));
    assert!(index.conversations()[0].cwd_missing);
    fs::create_dir_all(&cwd).unwrap();
    assert!(index.refresh(&tools, &roots));
    assert!(!index.conversations()[0].cwd_missing);
    assert_eq!(
        index.conversations()[0].project_root,
        project_root(&link.join("project"), &normalize_roots(&roots))
    );
    fs::remove_dir_all(home).unwrap();
}
use super::*;
#[test]
fn cache_wire_and_missing_cwd() {
    let home = std::env::temp_dir().join(format!("u9k-index-{}", std::process::id()));
    let bucket = home.join(".omp/agent/sessions/bucket");
    fs::create_dir_all(&bucket).unwrap();
    let path = bucket.join("date_id.jsonl");
    fs::write(&path,"{\"type\":\"session\",\"id\":\"id\",\"cwd\":\"/synthetic/nonexistent/cwd\",\"title\":\"metadata title\",\"timestamp\":\"2026-10-03T00:00:00Z\"}\n").unwrap();
    let mut index = Index::new(home.clone());
    let tools = crate::config::builtins();
    assert!(index.refresh(&tools, &[]));
    assert!(!index.refresh(&tools, &[]));
    assert_eq!(index.conversations().len(), 1);
    let c = &index.conversations()[0];
    assert!(c.cwd_missing);
    let wire = serde_json::to_value(c.entry(None, false)).unwrap();
    for key in [
        "projectId",
        "createdAt",
        "updatedAt",
        "cwdMissing",
        "runningItemId",
        "maybeOpenElsewhere",
    ] {
        assert!(wire.get(key).is_some());
    }
    assert!(wire.get("projectRoot").is_none());
    fs::remove_file(path).unwrap();
    assert!(index.refresh(&tools, &[]));
    assert!(index.conversations().is_empty());
    fs::remove_dir_all(home).unwrap();
}
#[test]
#[ignore = "read-only local-store scan; prints counts and timing, never titles or messages"]
fn real_local_store_counts() {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let mut index = Index::new(home);
    let start = std::time::Instant::now();
    index.refresh(&crate::config::builtins(), &[]);
    let mut counts = BTreeMap::<String, usize>::new();
    let mut projects = BTreeMap::<(String, PathBuf), usize>::new();
    for c in index.conversations() {
        *counts.entry(c.tool.clone()).or_default() += 1;
        *projects
            .entry((c.tool.clone(), c.project_root.clone()))
            .or_default() += 1;
    }
    for t in ["omp", "claude", "codex", "pi"] {
        println!("{t}: {} conversations", counts.get(t).copied().unwrap_or(0));
    }
    for ((tool, root), count) in projects {
        println!("{tool} project {}: {count}", root.display());
    }
    println!(
        "scan {:?}; {} store errors",
        start.elapsed(),
        index.errors().len()
    );
}
use crate::config::builtins;
use serde_json::json;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
/// A unique, canonical scratch directory (macOS: /var -> /private/var), removed on drop.
pub(super) struct Scratch(pub(super) PathBuf);
impl Scratch {
    pub(super) fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "u9k-discovery-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    /// Writes JSONL records (one per value) to a path relative to the scratch root.
    pub(super) fn jsonl(&self, relative: &str, records: &[serde_json::Value]) -> PathBuf {
        let text: String = records.iter().map(|r| format!("{r}\n")).collect();
        self.write(relative, &text)
    }
    pub(super) fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
    pub(super) fn dir(&self, relative: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn set_mtime(path: &Path, secs: u64) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}
fn omp_session(id: &str, cwd: &Path, title: Option<&str>) -> serde_json::Value {
    json!({"type": "session", "id": id, "cwd": cwd, "title": title})
}
fn tool(id: &str) -> ToolSpec {
    builtins().into_iter().find(|t| t.id == id).unwrap()
}
/// A project folder that is its own git root, whatever the scratch parent contains.
fn project(home: &Scratch) -> PathBuf {
    home.dir("project/.git");
    home.0.join("project")
}
fn summary(index: &Index) -> Vec<(&str, &str, &str, i64)> {
    index
        .conversations()
        .iter()
        .map(|c| {
            (
                c.tool.as_str(),
                c.id.as_str(),
                c.title.as_str(),
                c.updated_at_ms,
            )
        })
        .collect()
}

#[test]
fn windows_return_only_whole_tail_records_after_the_prefix() {
    let dir = Scratch::new("windows");
    let lines = dir.write("lines.jsonl", "AAAAAAAAA\nBBBBBBBBB\nCCCCCCCCC\n");
    let w = windows(&lines, 64, 8).unwrap();
    assert_eq!(w.prefix.len(), 30);
    assert!(w.tail.is_empty(), "a file within the prefix has no tail");
    // The tail starts exactly on a record boundary: keep that record.
    let w = windows(&lines, 5, 10).unwrap();
    assert_eq!(w.prefix, b"AAAAA");
    assert_eq!(w.tail, b"CCCCCCCCC\n");
    // The tail starts inside a record: drop the partial record.
    let w = windows(&lines, 5, 15).unwrap();
    assert_eq!(w.tail, b"CCCCCCCCC\n");
    // The tail never repeats bytes already in the prefix.
    let w = windows(&lines, 25, 10).unwrap();
    assert_eq!(w.prefix.len(), 25);
    assert!(w.tail.is_empty());
    // Without a prefix the whole file is tail, including its first record.
    let w = windows(&lines, 0, 64).unwrap();
    assert!(w.prefix.is_empty());
    assert_eq!(w.tail, b"AAAAAAAAA\nBBBBBBBBB\nCCCCCCCCC\n");
    let one_line = dir.write("one.jsonl", &"x".repeat(30));
    assert!(windows(&one_line, 5, 10).unwrap().tail.is_empty());
    assert_eq!(
        windows(&dir.0.join("missing.jsonl"), 5, 10)
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::NotFound
    );
}

#[test]
fn model_reads_default_changes_and_assistant_messages() {
    let m = |v: serde_json::Value| model(&metadata::decode(v.to_string().as_bytes()).unwrap());
    assert_eq!(
        m(json!({"type": "model_change", "model": "p/m"})).as_deref(),
        Some("p/m")
    );
    assert_eq!(
        m(json!({"type": "model_change", "modelId": "m", "provider": "p"})).as_deref(),
        Some("p/m")
    );
    // An id that already names its route is not prefixed again.
    assert_eq!(
        m(json!({"type": "model_change", "modelId": "x/m", "provider": "p"})).as_deref(),
        Some("x/m")
    );
    assert_eq!(
        m(json!({"type": "model_change", "role": "default", "model": "d"})).as_deref(),
        Some("d")
    );
    assert_eq!(
        m(json!({"type": "model_change", "role": "smol", "model": "s"})),
        None
    );
    assert_eq!(
        m(json!({"type": "message", "message": {"role": "assistant", "model": "m", "provider": "p"}}))
            .as_deref(),
        Some("p/m")
    );
    assert_eq!(
        m(json!({"type": "message", "message": {"role": "assistant", "model": "m"}})).as_deref(),
        Some("m")
    );
    assert_eq!(
        m(json!({"type": "message", "message": {"role": "user", "model": "m"}})),
        None
    );
    assert_eq!(
        m(json!({"type": "message", "message": {"role": "assistant"}})),
        None
    );
    assert_eq!(m(json!({"type": "session", "model": "m"})), None);
}

#[test]
fn timestamps_parse_rfc3339_or_fall_back_to_zero() {
    assert_eq!(timestamp(Some("2026-10-03T00:00:00Z")), 1_790_985_600_000);
    assert_eq!(
        timestamp(Some("2026-10-03T00:00:00+02:00")),
        1_790_978_400_000
    );
    assert_eq!(timestamp(Some("2026-10-03")), 0);
    assert_eq!(timestamp(None), 0);
}

#[test]
fn conversations_need_an_absolute_cwd_and_an_id_and_get_a_dated_fallback_title() {
    let path = Path::new("/store/s.jsonl");
    let make = |id: &str, cwd: &str, title: Option<&str>, created: i64| {
        conversation(
            "omp",
            path,
            id.into(),
            cwd.into(),
            title.map(Into::into),
            created,
            7,
            None,
        )
    };
    assert!(make("id", "relative/cwd", Some("t"), 0).is_none());
    assert!(make("", "/cwd", Some("t"), 0).is_none());
    let c = make("id", "/cwd", Some("Title"), 0).unwrap();
    assert_eq!(
        (c.title.as_str(), c.updated_at_ms, c.cwd_missing),
        ("Title", 7, false)
    );
    assert_eq!(
        (c.cwd.as_path(), c.project_root.as_path()),
        (Path::new("/cwd"), Path::new("/cwd"))
    );
    assert_eq!(
        make("id", "/cwd", Some("  "), 1_790_985_600_000)
            .unwrap()
            .title,
        "Untitled · 2026-10-03"
    );
    assert_eq!(
        make("id", "/cwd", None, i64::MAX).unwrap().title,
        "Untitled · unknown date"
    );
}

#[test]
fn store_dirs_watch_each_store_once_and_codex_from_its_parent() {
    let home = Path::new("/home/u");
    let mut tools = builtins();
    let mut alias = tool("claude");
    alias.id = "claude-work".into();
    tools.push(alias);
    assert_eq!(
        store_dirs(home, &tools),
        [
            home.join(".omp/agent/sessions"),
            home.join(".claude/projects"),
            home.join(".codex"),
            home.join(".pi/agent/sessions"),
        ]
    );
    assert!(store_dirs(home, &[tool("shell"), tool("neovim")]).is_empty());
}

#[test]
fn refresh_merges_every_store_newest_first() {
    let home = Scratch::new("merge");
    let cwd = project(&home);
    let omp = home.jsonl(
        ".omp/agent/sessions/b/2026_omp1.jsonl",
        &[omp_session("omp1", &cwd, Some("OMP chat"))],
    );
    let claude = home.jsonl(
        ".claude/projects/p/c1.jsonl",
        &[json!({"type": "assistant", "sessionId": "c1", "cwd": cwd, "message": {"model": "claude-x"}})],
    );
    let codex_meta = |id: &str| json!({"type": "session_meta", "payload": {"id": id, "cwd": cwd, "source": "cli"}});
    let codex = home.jsonl(
        ".codex/sessions/2026/10/03/rollout-x1.jsonl",
        &[codex_meta("x1")],
    );
    home.jsonl(
        ".codex/session_index.jsonl",
        &[json!({"id": "x1", "thread_name": "Codex title"})],
    );
    let pi = home.jsonl(
        ".pi/agent/sessions/p/s.jsonl",
        &[
            json!({"type": "session", "id": "pi1", "cwd": cwd}),
            json!({"type": "session_info", "name": "Pi title"}),
        ],
    );
    // Not collected: deeper than Codex's YYYY/MM/DD layout, or not JSONL.
    home.jsonl(
        ".codex/sessions/2026/10/03/nested/rollout-x2.jsonl",
        &[codex_meta("x2")],
    );
    home.jsonl(
        ".omp/agent/sessions/b/notes.txt",
        &[omp_session("txt", &cwd, Some("Text"))],
    );
    for (path, secs) in [
        (&omp, 1_000),
        (&claude, 3_000),
        (&codex, 2_000),
        (&pi, 4_000),
    ] {
        set_mtime(path, secs);
    }
    let mut index = Index::new(home.0.clone());
    assert!(index.refresh(&builtins(), &[]));
    assert!(index.errors().is_empty(), "{:?}", index.errors());
    assert_eq!(
        summary(&index),
        [
            ("pi", "pi1", "Pi title", 4_000_000),
            ("claude", "c1", "Untitled · 1970-01-01", 3_000_000),
            ("codex", "x1", "Codex title", 2_000_000),
            ("omp", "omp1", "OMP chat", 1_000_000),
        ]
    );
    for c in index.conversations() {
        assert_eq!((&c.project_root, c.cwd_missing), (&cwd, false));
    }
    assert_eq!(index.conversations()[1].model.as_deref(), Some("claude-x"));
    assert!(!index.refresh(&builtins(), &[]));
    // Only tools with a store are scanned.
    assert!(index.refresh(&[tool("shell")], &[]));
    assert!(index.conversations().is_empty());
}

#[test]
fn equal_update_times_order_by_path_then_tool() {
    let home = Scratch::new("ties");
    let cwd = home.dir("project");
    for id in ["b", "a"] {
        let path = home.jsonl(
            &format!(".omp/agent/sessions/x/2026_{id}.jsonl"),
            &[omp_session(id, &cwd, Some(id))],
        );
        set_mtime(&path, 5_000);
    }
    let mut second = tool("omp");
    second.id = "omp-alt".into();
    let mut index = Index::new(home.0.clone());
    index.refresh(&[second, tool("omp")], &[]);
    let order: Vec<_> = index
        .conversations()
        .iter()
        .map(|c| (c.id.as_str(), c.tool.as_str()))
        .collect();
    assert_eq!(
        order,
        [
            ("a", "omp"),
            ("a", "omp-alt"),
            ("b", "omp"),
            ("b", "omp-alt")
        ]
    );
}

#[test]
fn unchanged_size_and_mtime_reuse_the_cached_read() {
    let home = Scratch::new("cache");
    let cwd = home.dir("project");
    let relative = ".omp/agent/sessions/x/2026_id.jsonl";
    let path = home.jsonl(relative, &[omp_session("id", &cwd, Some("aaaa"))]);
    set_mtime(&path, 1_000);
    let tools = [tool("omp")];
    let mut index = Index::new(home.0.clone());
    index.refresh(&tools, &[]);
    home.jsonl(relative, &[omp_session("id", &cwd, Some("bbbb"))]);
    set_mtime(&path, 1_000);
    assert!(!index.refresh(&tools, &[]));
    assert_eq!(index.conversations()[0].title, "aaaa");
    set_mtime(&path, 2_000);
    assert!(index.refresh(&tools, &[]));
    assert_eq!(index.conversations()[0].title, "bbbb");
    home.jsonl(relative, &[omp_session("id", &cwd, Some("a longer title"))]);
    set_mtime(&path, 2_000);
    assert!(index.refresh(&tools, &[]));
    assert_eq!(index.conversations()[0].title, "a longer title");
}

#[test]
fn roots_keep_only_conversations_inside_them_grouped_by_git_root() {
    let home = Scratch::new("roots");
    let work = home.dir("work");
    home.dir("work/.git");
    let inside = home.dir("work/app/src");
    let outside = home.dir("elsewhere");
    home.jsonl(
        ".omp/agent/sessions/x/2026_in.jsonl",
        &[omp_session("in", &inside, Some("in"))],
    );
    home.jsonl(
        ".omp/agent/sessions/x/2026_out.jsonl",
        &[omp_session("out", &outside, Some("out"))],
    );
    let tools = [tool("omp")];
    let mut index = Index::new(home.0.clone());
    index.refresh(&tools, &[]);
    assert_eq!(index.conversations().len(), 2);
    assert!(index.refresh(&tools, std::slice::from_ref(&work)));
    assert_eq!(index.conversations().len(), 1);
    let c = &index.conversations()[0];
    assert_eq!((c.id.as_str(), &c.project_root), ("in", &work));
}

#[test]
fn store_errors_are_reported_in_order_and_count_as_changes() {
    let home = Scratch::new("errors");
    // A store path that is not a directory cannot be listed.
    let pi_store = home.write(".pi/agent/sessions", "not a directory");
    let index_file = home.write(".codex/session_index.jsonl", "");
    File::options()
        .write(true)
        .open(&index_file)
        .unwrap()
        .set_len(16 * 1024 * 1024 + 1)
        .unwrap();
    let tools = builtins();
    let mut index = Index::new(home.0.clone());
    assert!(index.refresh(&tools, &[]));
    let errors: Vec<_> = index
        .errors()
        .iter()
        .map(|e| (e.tool.as_str(), e.path.clone()))
        .collect();
    assert_eq!(
        errors,
        [("codex", index_file.clone()), ("pi", pi_store.clone())]
    );
    assert!(index.errors()[0].message.contains("resource limit"));
    assert!(!index.refresh(&tools, &[]));
    fs::remove_file(&pi_store).unwrap();
    assert!(index.refresh(&tools, &[]));
    assert_eq!(index.errors().len(), 1);
}

#[test]
fn omp_lookup_uses_the_index_then_a_bounded_store_scan() {
    let home = Scratch::new("lookup");
    let cwd = project(&home);
    let bucket = ".omp/agent/sessions/x";
    home.jsonl(
        &format!("{bucket}/2026_listed.jsonl"),
        &[omp_session("listed", &cwd, Some("Listed"))],
    );
    // A fresh session with no title or reply yet is hidden from the palette.
    let stub = home.jsonl(
        &format!("{bucket}/2026_stub.jsonl"),
        &[omp_session("stub", &cwd, None)],
    );
    home.jsonl(
        &format!("{bucket}/2026_renamed.jsonl"),
        &[omp_session("other", &cwd, None)],
    );
    home.jsonl(
        ".claude/projects/p/cl.jsonl",
        &[json!({"type": "assistant", "sessionId": "cl", "cwd": cwd})],
    );
    let mut index = Index::new(home.0.clone());
    index.refresh(&builtins(), &[]);
    assert_eq!(index.conversations().len(), 2);
    assert_eq!(index.omp_by_session_id("listed").unwrap().title, "Listed");
    let found = index.omp_by_session_id("stub").unwrap();
    assert_eq!(
        (&found.path, &found.project_root, found.cwd_missing),
        (&stub, &cwd, false)
    );
    for id in ["renamed", "missing", "", "x/2026_stub", "cl"] {
        assert!(index.omp_by_session_id(id).is_none(), "{id:?}");
    }
}

#[test]
fn unreadable_session_files_are_reported_without_hiding_the_rest() {
    use std::os::unix::fs::PermissionsExt;
    let home = Scratch::new("unreadable");
    let cwd = project(&home);
    let bucket = ".omp/agent/sessions/x";
    home.jsonl(
        &format!("{bucket}/2026_ok.jsonl"),
        &[omp_session("ok", &cwd, Some("Readable"))],
    );
    let locked = home.jsonl(
        &format!("{bucket}/2026_locked.jsonl"),
        &[omp_session("locked", &cwd, Some("Locked"))],
    );
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if File::open(&locked).is_ok() {
        // Privileged runs (root) bypass file permissions; nothing to observe.
        return;
    }
    let mut index = Index::new(home.0.clone());
    index.refresh(&[tool("omp")], &[]);
    let ids: Vec<_> = index
        .conversations()
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(ids, ["ok"]);
    let errors: Vec<_> = index
        .errors()
        .iter()
        .map(|e| (e.tool.as_str(), &e.path))
        .collect();
    assert_eq!(errors, [("omp", &locked)]);
    // A direct lookup of that session reports the same file instead of guessing.
    assert!(index.omp_by_session_id("locked").is_none());
    assert_eq!(index.errors().len(), 2);
    assert_eq!(index.errors()[1].path, locked);
}

#[test]
fn bound_reads_stay_inside_the_tool_store_and_keep_the_saved_path() {
    let home = Scratch::new("bound");
    let cwd = project(&home);
    let claude = home.jsonl(
        ".claude/projects/p/c1.jsonl",
        &[json!({"type": "assistant", "sessionId": "c1", "cwd": cwd})],
    );
    let codex = home.jsonl(
        ".codex/sessions/2026/10/03/rollout.jsonl",
        &[json!({"type": "session_meta", "payload": {"id": "x1", "cwd": cwd, "source": "cli"}})],
    );
    let pi = home.jsonl(
        ".pi/agent/sessions/p/s.jsonl",
        &[
            json!({"type": "session", "id": "pi1", "cwd": home.0.join("gone")}),
            json!({"type": "session_info", "name": "Pi title"}),
        ],
    );
    let read = |tool_id: &str, id: &str, path: &Path, roots: &[PathBuf]| {
        read_bound(&tool(tool_id), id, path, &home.0, roots)
    };
    assert_eq!(
        read("shell", "c1", &claude, &[]).unwrap_err(),
        "This tool has no saved-conversation store."
    );
    assert!(read("claude", "c1", &home.0.join("missing.jsonl"), &[])
        .unwrap_err()
        .starts_with("Cannot read saved conversation"));
    // The OMP store does not exist yet, then exists without this file.
    assert!(read("omp", "c1", &claude, &[]).is_err());
    home.dir(".omp/agent/sessions");
    assert_eq!(
        read("omp", "c1", &claude, &[]).unwrap_err(),
        "The saved conversation is outside this tool's store."
    );
    // A symlink outside the store is authorized by its target but keeps its spelling.
    let link = home.0.join("link.jsonl");
    std::os::unix::fs::symlink(&claude, &link).unwrap();
    let c = read("claude", "c1", &link, &[]).unwrap();
    assert_eq!(
        (c.tool.as_str(), &c.path, &c.project_root),
        ("claude", &link, &cwd)
    );
    assert_eq!(read("codex", "x1", &codex, &[]).unwrap().id, "x1");
    let p = read("pi", "pi1", &pi, &[]).unwrap();
    assert_eq!((p.title.as_str(), p.cwd_missing), ("Pi title", true));
    assert!(read("claude", "c1", &claude, std::slice::from_ref(&cwd)).is_ok());
    assert_eq!(
        read("claude", "c1", &claude, &[home.0.join("other")]).unwrap_err(),
        "Directory is outside projects.roots."
    );
}
