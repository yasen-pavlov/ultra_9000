use super::*;
use std::fs;
#[test]
fn background_is_hidden() {
    let path = std::env::temp_dir().join(format!("u9k-claude-{}.jsonl", std::process::id()));
    fs::write(
        &path,
        "{\"type\":\"assistant\",\"sessionId\":\"s\",\"cwd\":\"/x\",\"sessionKind\":\"bg\"}\n",
    )
    .unwrap();
    assert!(read("claude", &path, 0).unwrap().is_none());
    fs::remove_file(path).unwrap();
}
use crate::discovery::tests::Scratch;
use serde_json::json;

#[test]
fn explicit_titles_win_and_the_last_assistant_model_is_kept() {
    let dir = Scratch::new("claude-read");
    let path = dir.jsonl(
        "s.jsonl",
        &[
            json!({"type": "user", "sessionId": "c1", "cwd": "/work", "timestamp": "2026-10-03T00:00:00Z", "message": {"content": "prompt"}}),
            json!({"type": "custom-title", "customTitle": "Mine"}),
            json!({"type": "assistant", "sessionId": "ignored", "cwd": "/other", "timestamp": "2026-10-04T00:00:00Z", "message": {"model": "claude-a"}}),
            json!({"type": "ai-title", "aiTitle": "Generated"}),
            json!({"type": "assistant", "message": {"model": "claude-b"}}),
        ],
    );
    let c = read("claude", &path, 3).unwrap().unwrap();
    assert_eq!(
        (
            c.tool.as_str(),
            c.id.as_str(),
            c.cwd.to_str(),
            c.title.as_str()
        ),
        ("claude", "c1", Some("/work"), "Mine")
    );
    assert_eq!(
        (c.model.as_deref(), c.created_at_ms, c.updated_at_ms),
        (Some("claude-b"), 1_790_985_600_000, 3)
    );
    let ai = dir.jsonl(
        "ai.jsonl",
        &[
            json!({"type": "user", "sessionId": "c2", "cwd": "/work"}),
            json!({"type": "ai-title", "aiTitle": "Generated"}),
        ],
    );
    // A titled conversation is listed even before any reply.
    assert_eq!(read("claude", &ai, 0).unwrap().unwrap().title, "Generated");
}

#[test]
fn prompt_only_sidechain_and_incomplete_sessions_are_hidden() {
    let dir = Scratch::new("claude-hidden");
    for records in [
        vec![
            json!({"type": "user", "sessionId": "c", "cwd": "/work", "message": {"content": "prompt"}}),
        ],
        vec![
            json!({"type": "user", "sessionId": "c", "cwd": "/work"}),
            json!({"type": "assistant", "isSidechain": true}),
        ],
        vec![json!({"type": "assistant", "cwd": "/work"})],
        vec![json!({"type": "assistant", "sessionId": "c"})],
    ] {
        let path = dir.jsonl("s.jsonl", &records);
        assert!(read("claude", &path, 0).unwrap().is_none(), "{records:?}");
    }
    let path = dir.jsonl(
        "untitled.jsonl",
        &[json!({"type": "assistant", "sessionId": "c", "cwd": "/work"})],
    );
    let c = read("claude", &path, 0).unwrap().unwrap();
    assert_eq!((c.title.as_str(), c.model), ("Untitled · 1970-01-01", None));
}
