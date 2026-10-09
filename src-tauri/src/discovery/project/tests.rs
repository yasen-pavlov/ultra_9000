use super::*;
#[test]
fn git_root_and_worktree_file() {
    // Resolved like project_root resolves cwd (macOS: /var -> /private/var).
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("u9k-git-{}", std::process::id()));
    std::fs::create_dir_all(root.join("a/b")).unwrap();
    std::fs::write(root.join(".git"), "gitdir: elsewhere").unwrap();
    assert_eq!(project_root(&root.join("a/b"), &[]), root);
    std::fs::remove_file(root.join(".git")).unwrap();
    std::fs::create_dir(root.join("a/.git")).unwrap();
    assert_eq!(project_root(&root.join("a/b"), &[]), root.join("a"));
    std::fs::remove_dir_all(root).unwrap();
}
