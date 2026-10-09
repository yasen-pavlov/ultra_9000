use super::*;
use std::{collections::BTreeMap, fs, io};
#[test]
fn guardian_hidden() {
    let path = std::env::temp_dir().join(format!("u9k-codex-{}.jsonl", std::process::id()));
    fs::write(&path,"{\"type\":\"session_meta\",\"payload\":{\"id\":\"s\",\"cwd\":\"/x\",\"source\":{\"subagent\":{\"other\":\"guardian\"}},\"thread_source\":\"guardian_review\"}}\n").unwrap();
    assert!(read("codex", &path, 0).unwrap().is_none());
    fs::remove_file(path).unwrap();
}
#[test]
fn title_index_bounds_total_bytes_and_entries() {
    use std::io::Write;
    let path = std::env::temp_dir().join(format!("u9k-codex-limits-{}.jsonl", std::process::id()));
    let file = fs::File::create(&path).unwrap();
    file.set_len(MAX_INDEX_BYTES + 1).unwrap();
    assert_eq!(
        titles(&path).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    let mut writer = io::BufWriter::new(fs::File::create(&path).unwrap());
    for n in 0..=MAX_TITLE_ENTRIES {
        writeln!(writer, "{{\"id\":\"{n}\",\"thread_name\":\"title\"}}").unwrap();
    }
    writer.flush().unwrap();
    assert_eq!(
        titles(&path).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    fs::write(
        &path,
        "{\"id\":\"a\",\"thread_name\":\"old\"}\n{\"id\":\"a\",\"thread_name\":\"new\"}\n",
    )
    .unwrap();
    assert_eq!(
        titles(&path).unwrap().get("a").map(String::as_str),
        Some("new")
    );
    fs::remove_file(path).unwrap();
}
use crate::discovery::tests::Scratch;
use serde_json::json;

fn pairs(map: BTreeMap<String, String>) -> Vec<(String, String)> {
    map.into_iter().collect()
}
fn pair(id: &str, title: &str) -> (String, String) {
    (id.into(), title.into())
}

#[test]
fn titles_skip_blank_malformed_and_overlong_lines() {
    let dir = Scratch::new("codex-titles");
    assert!(titles(&dir.0.join("missing.jsonl")).unwrap().is_empty());
    let overlong = format!(
        "{{\"id\":\"long\",\"thread_name\":\"{}\"}}",
        "x".repeat(100 * 1024)
    );
    let path = dir.write(
        "index.jsonl",
        &format!(
            "{overlong}\n{{\"id\":\"a\",\"thread_name\":\"Alpha\"}}\nnot json\n{{\"id\":\"b\",\"thread_name\":\"  \"}}\n{{\"id\":\"c\",\"thread_name\":\"Gamma\"}}"
        ),
    );
    assert_eq!(
        pairs(titles(&path).unwrap()),
        [pair("a", "Alpha"), pair("c", "Gamma")]
    );
    // An overlong final record without a newline is skipped up to end of file.
    let path = dir.write(
        "tail.jsonl",
        &format!("{{\"id\":\"a\",\"thread_name\":\"Alpha\"}}\n{overlong}"),
    );
    assert_eq!(pairs(titles(&path).unwrap()), [pair("a", "Alpha")]);
}

#[test]
fn title_index_bounds_retained_bytes_and_releases_replaced_titles() {
    let dir = Scratch::new("codex-title-bytes");
    let name = "t".repeat(60 * 1024);
    let record = |id: &str| format!("{{\"id\":\"{id}\",\"thread_name\":\"{name}\"}}\n");
    // 150 distinct 60 KiB titles exceed the 8 MiB retention bound.
    let distinct: String = (0..150).map(|n| record(&n.to_string())).collect();
    let path = dir.write("distinct.jsonl", &distinct);
    assert_eq!(
        titles(&path).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    // The same volume renaming one thread retains only its latest title.
    let renamed: String = (0..150).map(|_| record("same")).collect();
    let path = dir.write("renamed.jsonl", &renamed);
    assert_eq!(pairs(titles(&path).unwrap()), [pair("same", &name)]);
}

#[test]
fn only_interactive_rollouts_are_listed() {
    let dir = Scratch::new("codex-sources");
    for (n, (payload, visible)) in [
        (json!({"source": "cli"}), true),
        (json!({"source": "codex-tui"}), true),
        (json!({"source": "user"}), true),
        (json!({}), true),
        (json!({"source": "cli", "thread_source": "user"}), true),
        (json!({"source": "exec"}), false),
        (json!({"source": {"subagent": "review"}}), false),
        (json!({"source": "cli", "thread_source": "subagent"}), false),
    ]
    .into_iter()
    .enumerate()
    {
        let mut payload = payload;
        payload["id"] = json!("s");
        payload["cwd"] = json!("/work");
        let path = dir.jsonl(
            &format!("{n}.jsonl"),
            &[json!({"type": "session_meta", "payload": payload.clone()})],
        );
        assert_eq!(
            read("codex", &path, 0).unwrap().is_some(),
            visible,
            "{payload}"
        );
    }
}

#[test]
fn rollout_metadata_maps_to_a_conversation() {
    let dir = Scratch::new("codex-read");
    let path = dir.jsonl(
        "rollout.jsonl",
        &[
            json!({"type": "session_meta", "payload": {"session_id": "s1", "cwd": "/work", "timestamp": "2026-10-03T00:00:00Z", "source": "cli", "model_provider": "openai"}}),
            json!({"type": "turn_context", "payload": {"model": "gpt-a"}}),
            json!({"type": "session_meta", "payload": {"id": "later", "cwd": "/other"}}),
            json!({"type": "turn_context", "payload": {"cwd": "/work"}}),
            json!({"type": "turn_context", "payload": {"model": "gpt-b"}}),
        ],
    );
    let c = read("codex", &path, 9).unwrap().unwrap();
    assert_eq!(
        (
            c.tool.as_str(),
            c.id.as_str(),
            c.cwd.to_str(),
            c.model.as_deref()
        ),
        ("codex", "s1", Some("/work"), Some("gpt-b"))
    );
    assert_eq!(
        (c.title.as_str(), c.created_at_ms, c.updated_at_ms),
        ("Untitled · 2026-10-03", 1_790_985_600_000, 9)
    );
    for records in [
        vec![json!({"type": "turn_context", "payload": {"model": "gpt-a"}})],
        vec![json!({"type": "session_meta", "payload": {"id": "s", "source": "cli"}})],
        vec![json!({"type": "session_meta", "payload": {"cwd": "/work", "source": "cli"}})],
    ] {
        let path = dir.jsonl("partial.jsonl", &records);
        assert!(read("codex", &path, 0).unwrap().is_none(), "{records:?}");
    }
}
