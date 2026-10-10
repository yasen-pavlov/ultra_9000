use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
};
fn slot(title: &str) -> String {
    let mut s =
        serde_json::json!({"type":"title","v":1,"title":title,"updatedAt":"2026-10-03","pad":""})
            .to_string();
    let n = 255 - s.len();
    s = s.replace("\"pad\":\"\"", &format!("\"pad\":\"{}\"", " ".repeat(n)));
    s.push('\n');
    assert_eq!(s.len(), 256);
    s
}
#[test]
fn live_binding_keeps_new_stub() {
    let stub = b"{\"type\":\"session\",\"id\":\"stub\",\"cwd\":\"/synthetic/cwd\"}\n";
    assert!(
        parse_with_stubs("omp", Path::new("/fixture"), stub, b"", 0, false)
            .unwrap()
            .is_some()
    );
}
#[test]
fn slot_header_stub_and_model() {
    let header="{\"type\":\"session\",\"id\":\"id\",\"cwd\":\"/synthetic/cwd\",\"title\":\"old\",\"timestamp\":\"2026-10-03T00:00:00Z\"}\n";
    let data = format!(
        "{}{header}{{\"type\":\"model_change\",\"model\":\"provider/initial\"}}\n",
        slot("new")
    );
    let c = parse("omp", Path::new("/fixture"), data.as_bytes(), b"", 12)
        .unwrap()
        .unwrap();
    assert_eq!(c.title, "new");
    assert_eq!(c.cwd, PathBuf::from("/synthetic/cwd"));
    assert_eq!(c.model.as_deref(), Some("provider/initial"));
    let stub = b"{\"type\":\"session\",\"id\":\"stub\",\"cwd\":\"/synthetic/cwd\"}\n";
    assert!(parse("omp", Path::new("/fixture"), stub, b"", 0)
        .unwrap()
        .is_none());
    let tail=b"{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"model\":\"actual\",\"provider\":\"provider\"}}\n";
    assert!(parse("omp", Path::new("/fixture"), stub, tail, 0)
        .unwrap()
        .unwrap()
        .title
        .starts_with("Untitled"));
}
#[test]
fn conversation_model_is_the_latest_default_model_in_the_tail() {
    let path = std::env::temp_dir().join(format!("u9k-model-{}.jsonl", std::process::id()));
    let data=format!("{{\"type\":\"session\",\"id\":\"id\",\"cwd\":\"/synthetic/cwd\",\"title\":\"t\"}}\n{{\"type\":\"model_change\",\"model\":\"initial\"}}\n{}\n{{\"type\":\"model_change\",\"model\":\"final\"}}\n{{\"type\":\"model_change\",\"role\":\"fallback\",\"model\":\"ignored\"}}\n","x".repeat(80*1024));
    fs::write(&path, data).unwrap();
    let c = read("omp", &path, 0).unwrap().unwrap();
    assert_eq!(c.model.as_deref(), Some("final"));
    fs::remove_file(path).unwrap();
}
