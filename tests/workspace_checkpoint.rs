use std::path::Path;
use std::process::Command;

use exagent::workspace_checkpoint::{
    create_checkpoint, prune_checkpoints, restore_checkpoint, workspace_content_hash,
};
use tempfile::tempdir;

#[test]
fn dirty_repo_checkpoint_preserves_git_state_and_user_index() {
    let repo = tempdir().unwrap();
    init_repo(repo.path());
    write(repo.path().join("tracked.txt"), "base\n");
    git(repo.path(), ["add", "tracked.txt"]);
    git(repo.path(), ["commit", "-m", "initial"]);

    write(repo.path().join("tracked.txt"), "staged\n");
    git(repo.path(), ["add", "tracked.txt"]);
    write(repo.path().join("tracked.txt"), "worktree\n");
    write(repo.path().join("untracked.txt"), "untracked\n");

    let status_before = git_stdout(repo.path(), ["status", "--short"]);
    let head_before = git_stdout(repo.path(), ["rev-parse", "HEAD"]);
    let stash_before = git_stdout(repo.path(), ["stash", "list"]);
    let branches_before = git_stdout(repo.path(), ["branch", "--format=%(refname:short)"]);
    let cached_before = git_stdout(repo.path(), ["diff", "--cached", "--", "tracked.txt"]);
    let index_before = git_stdout(repo.path(), ["ls-files", "--stage"]);

    let checkpoint_id = create_checkpoint(repo.path())
        .unwrap()
        .expect("git repo should produce checkpoint");

    assert_eq!(
        git_stdout(repo.path(), ["status", "--short"]),
        status_before
    );
    assert_eq!(git_stdout(repo.path(), ["rev-parse", "HEAD"]), head_before);
    assert_eq!(git_stdout(repo.path(), ["stash", "list"]), stash_before);
    assert_eq!(
        git_stdout(repo.path(), ["branch", "--format=%(refname:short)"]),
        branches_before
    );
    assert_eq!(
        git_stdout(repo.path(), ["diff", "--cached", "--", "tracked.txt"]),
        cached_before
    );
    assert_eq!(
        git_stdout(repo.path(), ["ls-files", "--stage"]),
        index_before
    );
    git(
        repo.path(),
        [
            "show-ref",
            "--verify",
            &format!("refs/exagent/checkpoints/{checkpoint_id}"),
        ],
    );

    assert_eq!(
        git_stdout(
            repo.path(),
            ["show", &format!("{checkpoint_id}:tracked.txt")]
        ),
        "worktree\n"
    );
}

#[test]
fn checkpoint_captures_untracked_files() {
    let repo = tempdir().unwrap();
    init_repo(repo.path());
    write(repo.path().join("tracked.txt"), "base\n");
    git(repo.path(), ["add", "tracked.txt"]);
    git(repo.path(), ["commit", "-m", "initial"]);
    write(repo.path().join("untracked.txt"), "capture me\n");

    let checkpoint_id = create_checkpoint(repo.path()).unwrap().unwrap();

    assert_eq!(
        git_stdout(
            repo.path(),
            ["show", &format!("{checkpoint_id}:untracked.txt")]
        ),
        "capture me\n"
    );
}

#[test]
fn workspace_content_hash_is_stable_for_unchanged_tree() {
    let repo = tempdir().unwrap();
    init_repo(repo.path());
    write(repo.path().join("tracked.txt"), "base\n");
    git(repo.path(), ["add", "tracked.txt"]);
    git(repo.path(), ["commit", "-m", "initial"]);
    write(repo.path().join("untracked.txt"), "capture me\n");

    let first = workspace_content_hash(repo.path()).unwrap().unwrap();
    let second = workspace_content_hash(repo.path()).unwrap().unwrap();
    assert_eq!(first, second);

    write(repo.path().join("untracked.txt"), "changed\n");
    let changed = workspace_content_hash(repo.path()).unwrap().unwrap();
    assert_ne!(first, changed);
}

#[test]
fn restore_checkpoint_restores_content_and_removes_later_files_inside_root_only() {
    let parent = tempdir().unwrap();
    let repo_root = parent.path().join("repo");
    std::fs::create_dir(&repo_root).unwrap();
    init_repo(&repo_root);
    write(repo_root.join("tracked.txt"), "base\n");
    git(&repo_root, ["add", "tracked.txt"]);
    git(&repo_root, ["commit", "-m", "initial"]);

    write(repo_root.join("tracked.txt"), "checkpoint tracked\n");
    write(repo_root.join("untracked.txt"), "checkpoint untracked\n");
    let checkpoint_id = create_checkpoint(&repo_root).unwrap().unwrap();

    write(repo_root.join("tracked.txt"), "after tracked\n");
    std::fs::remove_file(repo_root.join("untracked.txt")).unwrap();
    write(repo_root.join("after.txt"), "remove me\n");
    std::fs::create_dir(repo_root.join("nested")).unwrap();
    write(
        repo_root.join("nested").join("after.txt"),
        "remove me too\n",
    );
    write(parent.path().join("outside.txt"), "do not remove\n");

    restore_checkpoint(&repo_root, &checkpoint_id).unwrap();

    assert_eq!(
        std::fs::read_to_string(repo_root.join("tracked.txt")).unwrap(),
        "checkpoint tracked\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo_root.join("untracked.txt")).unwrap(),
        "checkpoint untracked\n"
    );
    assert!(!repo_root.join("after.txt").exists());
    assert!(!repo_root.join("nested").join("after.txt").exists());
    assert!(repo_root.join(".git").join("HEAD").exists());
    assert_eq!(
        std::fs::read_to_string(parent.path().join("outside.txt")).unwrap(),
        "do not remove\n"
    );
}

#[test]
fn restore_checkpoint_preserves_ignored_files() {
    let repo = tempdir().unwrap();
    init_repo(repo.path());
    write(repo.path().join(".gitignore"), "*.log\n");
    write(repo.path().join("tracked.txt"), "base\n");
    git(repo.path(), ["add", ".gitignore", "tracked.txt"]);
    git(repo.path(), ["commit", "-m", "initial"]);
    write(repo.path().join("tracked.txt"), "checkpoint tracked\n");
    write(repo.path().join("before.log"), "ignored before\n");
    let checkpoint_id = create_checkpoint(repo.path()).unwrap().unwrap();

    write(
        repo.path().join(".gitignore"),
        "# logs are no longer ignored\n",
    );
    write(repo.path().join("tracked.txt"), "after tracked\n");
    write(repo.path().join("after.txt"), "remove me\n");
    write(repo.path().join("after.log"), "ignored after\n");

    restore_checkpoint(repo.path(), &checkpoint_id).unwrap();

    assert_eq!(
        std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
        "checkpoint tracked\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join(".gitignore")).unwrap(),
        "*.log\n"
    );
    assert!(!repo.path().join("after.txt").exists());
    assert_eq!(
        std::fs::read_to_string(repo.path().join("before.log")).unwrap(),
        "ignored before\n"
    );
    assert!(!repo.path().join("after.log").exists());
}

#[test]
fn restore_checkpoint_preserves_only_ignored_directories_from_checkpoint_manifest() {
    let repo = tempdir().unwrap();
    init_repo(repo.path());
    write(repo.path().join(".gitignore"), "cache*/\n");
    write(repo.path().join("tracked.txt"), "base\n");
    git(repo.path(), ["add", ".gitignore", "tracked.txt"]);
    git(repo.path(), ["commit", "-m", "initial"]);

    let before_dir = repo.path().join("cache_before");
    std::fs::create_dir(&before_dir).unwrap();
    write(before_dir.join("before.bin"), "ignored before\n");
    let checkpoint_id = create_checkpoint(repo.path()).unwrap().unwrap();

    write(
        before_dir.join("after.bin"),
        "ignored after in existing dir\n",
    );
    let after_dir = repo.path().join("cache_after");
    std::fs::create_dir(&after_dir).unwrap();
    write(after_dir.join("after.bin"), "ignored after\n");

    restore_checkpoint(repo.path(), &checkpoint_id).unwrap();

    assert_eq!(
        std::fs::read_to_string(before_dir.join("before.bin")).unwrap(),
        "ignored before\n"
    );
    assert!(!before_dir.join("after.bin").exists());
    assert!(!after_dir.exists());
}

#[test]
fn prune_checkpoints_by_count_removes_matching_manifest_refs() {
    let repo = tempdir().unwrap();
    init_repo(repo.path());
    write(repo.path().join("tracked.txt"), "base\n");
    git(repo.path(), ["add", "tracked.txt"]);
    git(repo.path(), ["commit", "-m", "initial"]);

    for index in 0..3 {
        write(
            repo.path().join("tracked.txt"),
            &format!("checkpoint {index}\n"),
        );
        create_checkpoint(repo.path()).unwrap().unwrap();
    }

    prune_checkpoints(repo.path(), 1, None).unwrap();

    let checkpoint_refs = git_lines(
        repo.path(),
        [
            "for-each-ref",
            "--format=%(refname)",
            "refs/exagent/checkpoints",
        ],
    );
    let manifest_refs = git_lines(
        repo.path(),
        [
            "for-each-ref",
            "--format=%(refname)",
            "refs/exagent/checkpoint-manifests",
        ],
    );
    assert_eq!(checkpoint_refs.len(), 1);
    assert_eq!(manifest_refs.len(), 1);

    let remaining_id = checkpoint_refs[0]
        .strip_prefix("refs/exagent/checkpoints/")
        .expect("checkpoint ref namespace");
    assert_eq!(
        manifest_refs[0],
        format!("refs/exagent/checkpoint-manifests/{remaining_id}")
    );
}

#[test]
fn restore_checkpoint_does_not_recurse_into_nested_git_repositories() {
    let repo = tempdir().unwrap();
    init_repo(repo.path());
    write(repo.path().join("tracked.txt"), "base\n");
    git(repo.path(), ["add", "tracked.txt"]);
    git(repo.path(), ["commit", "-m", "initial"]);

    let nested = repo.path().join("nested-repo");
    std::fs::create_dir(&nested).unwrap();
    init_repo(&nested);
    write(nested.join("nested.txt"), "nested base\n");
    git(&nested, ["add", "nested.txt"]);
    git(&nested, ["commit", "-m", "nested initial"]);
    let checkpoint_id = create_checkpoint(repo.path()).unwrap().unwrap();

    write(nested.join("after.txt"), "nested after checkpoint\n");
    write(repo.path().join("after.txt"), "remove me\n");

    restore_checkpoint(repo.path(), &checkpoint_id).unwrap();

    assert!(!repo.path().join("after.txt").exists());
    assert_eq!(
        std::fs::read_to_string(nested.join("after.txt")).unwrap(),
        "nested after checkpoint\n"
    );
    assert!(nested.join(".git").join("HEAD").exists());
}

#[test]
fn restore_checkpoint_does_not_recurse_into_gitlink_worktree_contents() {
    let repo = tempdir().unwrap();
    init_repo(repo.path());
    write(repo.path().join("tracked.txt"), "base\n");
    git(repo.path(), ["add", "tracked.txt"]);
    git(repo.path(), ["commit", "-m", "initial"]);
    git(
        repo.path(),
        [
            "update-index",
            "--add",
            "--cacheinfo",
            "160000",
            "1111111111111111111111111111111111111111",
            "vendor",
        ],
    );
    git(repo.path(), ["commit", "-m", "add gitlink"]);
    let checkpoint_id = create_checkpoint(repo.path()).unwrap().unwrap();

    let vendor = repo.path().join("vendor");
    std::fs::create_dir(&vendor).unwrap();
    write(vendor.join("after.txt"), "gitlink worktree content\n");
    write(repo.path().join("after.txt"), "remove me\n");

    restore_checkpoint(repo.path(), &checkpoint_id).unwrap();

    assert!(!repo.path().join("after.txt").exists());
    assert_eq!(
        std::fs::read_to_string(vendor.join("after.txt")).unwrap(),
        "gitlink worktree content\n"
    );
}

#[test]
fn restore_checkpoint_uses_literal_pathspec_for_subdirectory_workspace_roots() {
    let repo = tempdir().unwrap();
    init_repo(repo.path());
    let workspace = repo.path().join("literal[glob]");
    let sibling_matching_glob = repo.path().join("literalg");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&sibling_matching_glob).unwrap();
    write(workspace.join("tracked.txt"), "base\n");
    write(sibling_matching_glob.join("sibling.txt"), "sibling\n");
    git(repo.path(), ["add", "."]);
    git(repo.path(), ["commit", "-m", "initial"]);
    write(workspace.join("tracked.txt"), "checkpoint\n");
    let checkpoint_id = create_checkpoint(&workspace).unwrap().unwrap();

    write(workspace.join("tracked.txt"), "after\n");
    write(workspace.join("after.txt"), "remove me\n");

    restore_checkpoint(&workspace, &checkpoint_id).unwrap();

    assert_eq!(
        std::fs::read_to_string(workspace.join("tracked.txt")).unwrap(),
        "checkpoint\n"
    );
    assert!(!workspace.join("after.txt").exists());
    assert_eq!(
        std::fs::read_to_string(sibling_matching_glob.join("sibling.txt")).unwrap(),
        "sibling\n"
    );
}

#[test]
fn create_checkpoint_returns_none_outside_git_repo() {
    let dir = tempdir().unwrap();

    assert_eq!(create_checkpoint(dir.path()).unwrap(), None);
}

fn init_repo(path: &Path) {
    git(path, ["init"]);
    git(path, ["config", "user.name", "ExAgent Test"]);
    git(
        path,
        ["config", "user.email", "exagent-test@example.invalid"],
    );
}

fn write(path: impl AsRef<Path>, content: &str) {
    std::fs::write(path, content).unwrap();
}

fn git<const N: usize>(cwd: &Path, args: [&str; N]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git command failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout<const N: usize>(cwd: &Path, args: [&str; N]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git command failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn git_lines<const N: usize>(cwd: &Path, args: [&str; N]) -> Vec<String> {
    git_stdout(cwd, args)
        .lines()
        .map(|line| line.to_string())
        .collect()
}
