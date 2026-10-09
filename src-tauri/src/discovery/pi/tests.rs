use super::*;
use crate::discovery::tests::Scratch;
use serde_json::json;

#[test]
fn session_header_named_title_and_latest_model() {
    let dir = Scratch::new("pi-read");
    let path = dir.jsonl(
        "s.jsonl",
        &[
            json!({"type": "session", "id": "p1", "cwd": "/work", "timestamp": "2026-10-03T00:00:00Z"}),
            json!({"type": "message", "message": {"role": "user", "content": "first prompt"}}),
            json!({"type": "session_info", "name": "Draft"}),
            json!({"type": "model_change", "modelId": "m1", "provider": "p"}),
            json!({"type": "session", "id": "other", "cwd": "/other"}),
            json!({"type": "session_info", "name": "Final name"}),
            json!({"type": "message", "message": {"role": "assistant", "model": "m2", "provider": "p"}}),
        ],
    );
    let c = read("pi", &path, 5).unwrap().unwrap();
    assert_eq!(
        (
            c.tool.as_str(),
            c.id.as_str(),
            c.cwd.to_str(),
            c.title.as_str()
        ),
        ("pi", "p1", Some("/work"), "Final name")
    );
    assert_eq!(
        (c.model.as_deref(), c.created_at_ms, c.updated_at_ms),
        (Some("p/m2"), 1_790_985_600_000, 5)
    );
}

#[test]
fn unnamed_sessions_never_use_the_prompt_and_incomplete_headers_are_skipped() {
    let dir = Scratch::new("pi-untitled");
    let path = dir.jsonl(
        "s.jsonl",
        &[
            json!({"type": "session", "id": "p1", "cwd": "/work"}),
            json!({"type": "message", "message": {"role": "user", "content": "secret prompt"}}),
        ],
    );
    assert_eq!(
        read("pi", &path, 0).unwrap().unwrap().title,
        "Untitled · 1970-01-01"
    );
    for records in [
        vec![json!({"type": "session_info", "name": "No header"})],
        vec![json!({"type": "session", "cwd": "/work"})],
        vec![json!({"type": "session", "id": "p1"})],
    ] {
        let path = dir.jsonl("partial.jsonl", &records);
        assert!(read("pi", &path, 0).unwrap().is_none(), "{records:?}");
    }
    assert!(read("pi", &dir.0.join("missing.jsonl"), 0).is_err());
}
