use super::*;
#[test]
fn atomic_roundtrip_and_defaults() {
    let root = std::env::temp_dir().join(format!("u9k-state-{}", std::process::id()));
    let path = root.join("nested/state.json");
    assert!(load(&path).sidebar_expanded);
    let s = UiStateFile {
        sidebar_expanded: false,
        folded_projects: vec!["/project".into()],
        last_set: vec![],
    };
    save(&path, &s).unwrap();
    assert!(!load(&path).sidebar_expanded);
    assert_eq!(load(&path).folded_projects, s.folded_projects);
    fs::write(&path, "broken").unwrap();
    assert!(load(&path).sidebar_expanded);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn unreadable_state_falls_back_and_unwritable_state_reports() {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("u9k-state-unreadable-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    // A directory where the file belongs is not "missing": still defaults, never a panic.
    let loaded = load(&root);
    assert!(loaded.sidebar_expanded);
    assert!(loaded.folded_projects.is_empty() && loaded.last_set.is_empty());
    let blocker = root.join("file");
    fs::write(&blocker, "").unwrap();
    assert!(save(&blocker.join("state.json"), &UiStateFile::default()).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn last_set_and_missing_fields_use_the_camel_case_wire_format() {
    let state: UiStateFile = serde_json::from_str(
        r#"{"lastSet":[{"tool":"omp","title":"Fix","projectId":"/atlas","cwd":"/atlas/src",
            "conversationId":"c1","conversationPath":"/store/c1.jsonl"}]}"#,
    )
    .unwrap();
    assert!(state.sidebar_expanded, "absent fields keep their defaults");
    let entry = &state.last_set[0];
    assert_eq!(
        (entry.project_id.as_str(), entry.conversation_path.as_str()),
        ("/atlas", "/store/c1.jsonl")
    );
    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(json["lastSet"][0]["conversationId"], "c1");
    assert_eq!(json["foldedProjects"], serde_json::json!([]));
}
