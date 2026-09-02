//! Integration test: git sync conflict detection.
//!
//! Creates two clones of a bare repo with conflicting edits to the same file,
//! then asserts `run_merge` returns `conflicted: true` with the file listed.

use git_companion::git::sync::run_merge;
use std::process::Command;
use tempfile::TempDir;

fn git(cwd: &std::path::Path, args: &[&str]) -> std::process::Output {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .output()
        .expect("git spawn");
    if !output.status.success() {
        panic!(
            "git {:?} failed (exit {}): {}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    output
}

fn write(path: &std::path::Path, name: &str, content: &str) {
    std::fs::write(path.join(name), content).unwrap();
}

fn configure_git_user(cwd: &std::path::Path) {
    git(cwd, &["config", "user.email", "test@example.com"]);
    git(cwd, &["config", "user.name", "Test User"]);
    git(cwd, &["config", "commit.gpgsign", "false"]);
}

/// Build the conflict scenario: two clones share a bare remote.
///
/// Clone A:
///   - main: initial commit (a.txt = "hello\n")
///   - main: second commit (a.txt = "main version\n")
/// Clone B:
///   - other: starts at initial commit (a.txt = "hello\n")
///   - other: commit (a.txt = "other version\n")
///
/// Result: merging origin/main into other → a.txt conflict.
fn setup_conflict(tmp: &TempDir) -> std::path::PathBuf {
    let bare = tmp.path().join("bare.git");
    git(tmp.path(), &["init", "--bare", &bare.to_string_lossy()]);

    // Clone A: sets up main with two commits on a.txt
    let repo_main = tmp.path().join("repo_main");
    std::fs::create_dir(&repo_main).unwrap();
    git(&repo_main, &["init", "-b", "main"]);
    configure_git_user(&repo_main);
    git(
        &repo_main,
        &["remote", "add", "origin", &bare.to_string_lossy()],
    );

    write(&repo_main, "a.txt", "hello\n");
    git(&repo_main, &["add", "a.txt"]);
    git(&repo_main, &["commit", "-m", "initial"]);
    git(&repo_main, &["push", "-u", "origin", "main"]);

    // Second commit on main — incompatible change
    write(&repo_main, "a.txt", "main version\n");
    git(&repo_main, &["commit", "-am", "main: change a.txt"]);
    git(&repo_main, &["push", "-u", "origin", "main"]);

    // Clone B: checks out "other" branch at the initial commit (before main's second commit)
    let repo_other = tmp.path().join("repo_other");
    std::fs::create_dir(&repo_other).unwrap();
    git(&repo_other, &["init", "-b", "other"]);
    configure_git_user(&repo_other);
    git(
        &repo_other,
        &["remote", "add", "origin", &bare.to_string_lossy()],
    );
    git(&repo_other, &["fetch", "origin", "main"]);

    // Point "other" at origin/main~1 (the initial commit, before main's conflicting change)
    git(&repo_other, &["reset", "--hard", "origin/main~1"]);

    // Commit a conflicting change on other
    write(&repo_other, "a.txt", "other version\n");
    git(&repo_other, &["add", "a.txt"]);
    git(&repo_other, &["commit", "-m", "other: conflicting change"]);
    git(&repo_other, &["push", "-u", "origin", "other"]);

    repo_other
}

/// No-conflict scenario: main has a.txt, topic adds b.txt.
fn setup_no_conflict(tmp: &TempDir) -> std::path::PathBuf {
    let bare = tmp.path().join("bare.git");
    git(tmp.path(), &["init", "--bare", &bare.to_string_lossy()]);

    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    configure_git_user(&repo);
    git(&repo, &["remote", "add", "origin", &bare.to_string_lossy()]);

    write(&repo, "a.txt", "hello\n");
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "-m", "initial"]);
    git(&repo, &["push", "-u", "origin", "main"]);

    git(&repo, &["checkout", "-b", "topic"]);
    write(&repo, "b.txt", "different\n");
    git(&repo, &["add", "b.txt"]);
    git(&repo, &["commit", "-m", "add b.txt"]);
    git(&repo, &["push", "-u", "origin", "topic"]);

    repo
}

#[test]
fn sync_with_base_detects_conflict() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_other = setup_conflict(&tmp);

    // Verify pre-condition: a.txt differs between the branches.
    let content = std::fs::read_to_string(repo_other.join("a.txt")).unwrap();
    assert_eq!(
        content, "other version\n",
        "pre-condition: other should have 'other version'"
    );

    let result = run_merge(&repo_other, "main", None).expect("run_merge should not return Err");

    assert!(result.conflicted, "expected conflicted=true, got false");
    assert!(
        result.files.iter().any(|f| f == "a.txt"),
        "expected 'a.txt' in conflicted files, got {:?}",
        result.files
    );

    let diff_out = git(&repo_other, &["diff", "--name-only", "--diff-filter=U"]);
    let diff_str = String::from_utf8_lossy(&diff_out.stdout);
    let conflicted: Vec<_> = diff_str
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    assert!(
        conflicted.iter().any(|l| *l == "a.txt"),
        "git diff --name-only --diff-filter=U should list a.txt, got {:?}",
        conflicted
    );

    println!(
        "PASS: sync_with_base correctly detected conflict in {:?}",
        result.files
    );
}

#[test]
fn sync_with_base_succeeds_when_no_conflict() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = setup_no_conflict(&tmp);

    git(&repo, &["checkout", "topic"]);

    // Verify b.txt exists on topic (not on main).
    let b_exists = repo.join("b.txt").exists();
    assert!(b_exists, "pre-condition: b.txt should exist on topic");

    let result = run_merge(&repo, "main", None).expect("run_merge should not return Err");

    assert!(!result.conflicted, "expected conflicted=false, got true");
    assert!(
        result.files.is_empty(),
        "expected no conflicted files, got {:?}",
        result.files
    );

    println!("PASS: sync_with_base succeeded with no conflict");
}
