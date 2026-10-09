use super::*;
#[test]
fn truncated_content_retains_only_metadata() {
    let r=decode(b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"s\",\"cwd\":\"/x\",\"source\":\"cli\",\"base_instructions\":{\"text\":\"unfinished").unwrap();
    assert_eq!(r.kind, "session_meta");
    let p = r.payload.unwrap();
    assert_eq!(p.id.as_deref(), Some("s"));
    assert_eq!(p.cwd.as_deref(), Some("/x"));
}
