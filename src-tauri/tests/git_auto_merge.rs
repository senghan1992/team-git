//! End-to-end tests for `git::auto::auto_resolve_merge` — the one-click
//! "AI 자동 병합" flow. A real git repo with a conflicting merge is used;
//! the AI resolver is stubbed with closures (deterministic input).
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;

use tempfile::TempDir;

use git_companion::git::auto::{auto_resolve_merge, AutoResolveOptions, SideChoice};
use git_companion::git::sync::run_merge;
use git_companion::git::Target;

/// Serializes tests that mutate the global GC_BACKUP_DIR env var.
static BACKUP_LOCK: Mutex<()> = Mutex::new(());

// ── Helpers (mirror tests/git_sync_conflict.rs) ─────────────────────────────

fn git(dir: &Path, args: &[&str]) -> Output {
    let out = Command::new("git")
        .current_dir(dir)
        .env("LC_ALL", "C.UTF-8")
        .args(args)
        .output()
        .expect("failed to spawn git");
    if !out.status.success() {
        panic!(
            "git {:?} failed in {:?}:\nstdout: {}\nstderr: {}",
            args,
            dir,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    out
}

fn write(dir: &Path, file: &str, contents: &str) {
    std::fs::write(dir.join(file), contents).unwrap();
}

fn configure_git_user(dir: &Path) {
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test User"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

/// main and feature branches both change `a.txt` → `git merge` crashes
/// into a conflict (the exact scenario from the user's report).
fn setup_conflict(tmp: &TempDir) -> PathBuf {
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

    git(&repo, &["checkout", "-b", "feature"]);
    write(&repo, "a.txt", "feature version\n");
    git(&repo, &["commit", "-am", "feature: change a.txt"]);
    git(&repo, &["push", "-u", "origin", "feature"]);

    git(&repo, &["checkout", "main"]);
    write(&repo, "a.txt", "main version\n");
    git(&repo, &["commit", "-am", "main: change a.txt"]);
    git(&repo, &["push", "-u", "origin", "main"]);

    repo
}

fn local_target(repo: &Path) -> Target {
    Target::Local(repo.to_path_buf())
}

/// Slug used by `backup_root` for a repo path.
fn backup_slug(repo: &Path) -> String {
    repo.to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn ai_disabled(
) -> impl Fn(&git_companion::git::merge::ConflictDetail) -> Result<String, git_companion::error::AppError>
{
    |_| {
        Err(git_companion::error::AppError::Config(
            "AI 충돌 해결이 비활성화되어 있습니다.".into(),
        ))
    }
}

/// AI를 끈 상태에서 사용자가 "규칙 기반으로 한쪽 골라라"를 명시적으로 요청한
/// 설정 — `commands::auto`가 `ai.enabled == false`일 때 만드는 옵션과 같다.
fn rule_based_opts() -> AutoResolveOptions {
    AutoResolveOptions {
        binary_strategy: SideChoice::Theirs,
        text_fallback: Some(SideChoice::Theirs),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn auto_resolve_commits_merge_via_deterministic_fallback() {
    let _guard = BACKUP_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let repo = setup_conflict(&tmp);
    // Isolate this test's backups from other tests running in parallel.
    std::env::set_var("GC_BACKUP_DIR", tmp.path().join("backups"));

    // Simulate the crash: the user already started the merge and it conflicted.
    let result = run_merge(&repo, "feature", None).expect("merge should start");
    assert!(result.conflicted, "fixture should conflict on a.txt");

    let report = auto_resolve_merge(&local_target(&repo), &rule_based_opts(), ai_disabled())
        .expect("auto resolve should succeed");

    assert!(!report.resolved.is_empty(), "a.txt should be resolved");
    assert_eq!(
        report.resolved[0].method, "theirs",
        "rule-based mode was explicitly requested → pick theirs"
    );
    assert!(report.remaining.is_empty(), "no conflicts may remain");
    assert!(report.committed, "all resolved → merge must be committed");
    assert!(
        report.backup_id.is_some(),
        "a resolution run must always back up first"
    );

    // Final state: no markers, no merge in progress, commit on top of main.
    let content = std::fs::read_to_string(repo.join("a.txt")).unwrap();
    assert!(
        !content.contains("<<<<<<<")
            && !content.contains("=======")
            && !content.contains(">>>>>>>"),
        "staged file must never contain conflict markers: {content}"
    );
    assert_eq!(
        content, "feature version\n",
        "fallback theirs == feature's text"
    );

    let status = git(&repo, &["status", "--porcelain"]);
    assert!(status.stdout.is_empty(), "tree must be clean after commit");

    let log = git(&repo, &["log", "-1", "--format=%s"]);
    let subject = String::from_utf8_lossy(&log.stdout);
    assert!(
        subject.contains("브렌치 병합"),
        "commit message should follow the team convention (<branch> 브렌치 병합), got {subject}"
    );

    // Backup of the conflicted working file must exist locally.
    let backups = git_companion::git::auto::list_backups(&local_target(&repo)).unwrap();
    assert_eq!(backups.len(), 1);
    assert!(
        backups[0].files.contains(&"a.txt".to_string()),
        "backup should list a.txt, got {:?}",
        backups[0].files
    );
}

#[test]
fn auto_resolve_uses_ai_result_when_valid() {
    let _guard = BACKUP_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let repo = setup_conflict(&tmp);
    std::env::set_var("GC_BACKUP_DIR", tmp.path().join("backups"));
    let result = run_merge(&repo, "feature", None).unwrap();
    assert!(result.conflicted);

    let report = auto_resolve_merge(&local_target(&repo), &AutoResolveOptions::default(), |_| {
        Ok("ai merged content\n".to_string())
    })
    .unwrap();

    assert_eq!(report.resolved.len(), 1);
    assert_eq!(report.resolved[0].method, "ai");
    assert!(report.committed);
    // strip_code_fence trims the body (trailing newline removed).
    assert_eq!(
        std::fs::read_to_string(repo.join("a.txt")).unwrap(),
        "ai merged content"
    );
}

#[test]
fn auto_resolve_rejects_marked_ai_output_and_falls_back_in_rule_based_mode() {
    let _guard = BACKUP_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let repo = setup_conflict(&tmp);
    std::env::set_var("GC_BACKUP_DIR", tmp.path().join("backups"));
    let result = run_merge(&repo, "feature", None).unwrap();
    assert!(result.conflicted);

    // AI returns a body still containing markers → must be rejected (safety).
    let report = auto_resolve_merge(&local_target(&repo), &rule_based_opts(), |_| {
        Ok("<<<<<<< HEAD\nmain\n=======\nfeature\n>>>>>>> feature\n".to_string())
    })
    .unwrap();

    assert_eq!(report.resolved[0].method, "theirs", "fallback must be used");
    assert!(report.committed);
    assert!(
        !std::fs::read_to_string(repo.join("a.txt"))
            .unwrap()
            .contains("<<<<<<<"),
        "markers must never survive"
    );
}

/// 데이터 손실 방지: AI가 쓸 수 없는 결과를 내놓았고 **양쪽이 모두 고친**
/// 텍스트 파일이라면, 통째로 한쪽을 골라 커밋/푸시해 버리면 팀원의 커밋이
/// 조용히 사라진다. 그래서 기본값(`text_fallback: None`)에서는 그 파일을
/// 충돌 상태로 남기고 사람에게 넘긴다.
#[test]
fn auto_resolve_leaves_both_sides_changed_file_for_a_human_when_ai_fails() {
    let _guard = BACKUP_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let repo = setup_conflict(&tmp);
    std::env::set_var("GC_BACKUP_DIR", tmp.path().join("backups"));
    let result = run_merge(&repo, "feature", None).unwrap();
    assert!(result.conflicted);

    let report = auto_resolve_merge(
        &local_target(&repo),
        &AutoResolveOptions::default(),
        ai_disabled(),
    )
    .unwrap();

    assert!(
        report.resolved.is_empty(),
        "nothing may be auto-resolved, got {:?}",
        report.resolved
    );
    assert_eq!(
        report.remaining,
        vec!["a.txt".to_string()],
        "the file must stay conflicted for manual resolution"
    );
    assert!(
        !report.committed,
        "a merge that lost a side's work must never be committed"
    );
    assert!(
        report.backup_id.is_some(),
        "the original is still backed up before anything is attempted"
    );
    // 병합은 계속 진행 중이어야 한다 — 병합 센터에서 이어서 끝낼 수 있게.
    assert!(git_companion::git::merge::merge_in_progress(&local_target(&repo)).unwrap());
    // 양쪽 내용이 워킹 트리에 그대로 남아 있어야 한다.
    let content = std::fs::read_to_string(repo.join("a.txt")).unwrap();
    assert!(
        content.contains("main version"),
        "ours must survive: {content}"
    );
    assert!(
        content.contains("feature version"),
        "theirs must survive: {content}"
    );
}

#[test]
fn auto_resolve_handles_binary_conflict_with_side_choice() {
    let _guard = BACKUP_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let repo = setup_conflict(&tmp);
    std::env::set_var("GC_BACKUP_DIR", tmp.path().join("backups"));

    // Make a.txt binary on both sides so git reports a binary conflict.
    git(&repo, &["checkout", "feature"]);
    std::fs::write(repo.join("a.txt"), b"\x00\x01\x02binary-feature\x00").unwrap();
    git(&repo, &["commit", "-am", "feature: binary"]);

    git(&repo, &["checkout", "main"]);
    std::fs::write(repo.join("a.txt"), b"\x00\x01\x02binary-main\x00").unwrap();
    git(&repo, &["commit", "-am", "main: binary"]);
    git(&repo, &["push", "-u", "origin", "main"]);
    git(&repo, &["push", "-u", "origin", "feature"]);

    let result = run_merge(&repo, "feature", None).unwrap();
    assert!(result.conflicted, "binary files must conflict");

    let report = auto_resolve_merge(
        &local_target(&repo),
        &AutoResolveOptions::default(), // binary_strategy = theirs
        ai_disabled(),
    )
    .unwrap();

    assert_eq!(report.resolved.len(), 1);
    assert_eq!(report.resolved[0].method, "theirs");
    assert!(report.committed);
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"\x00\x01\x02binary-feature\x00",
        "theirs == feature's binary content"
    );
}

#[test]
fn auto_resolve_refuses_without_merge_in_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = setup_conflict(&tmp);
    // No merge was started — nothing to resolve.
    let err = auto_resolve_merge(
        &local_target(&repo),
        &AutoResolveOptions::default(),
        ai_disabled(),
    )
    .expect_err("should refuse when nothing is being merged");
    let msg = err.to_string();
    assert!(
        msg.contains("진행 중인 병합이 없습니다"),
        "expected Korean refusal message, got {msg}"
    );
}

#[test]
fn backup_restore_roundtrip_recovers_pre_resolution_content() {
    let _guard = BACKUP_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let repo = setup_conflict(&tmp);
    let backups_dir = tmp.path().join("backups");
    std::env::set_var("GC_BACKUP_DIR", &backups_dir);
    let result = run_merge(&repo, "feature", None).unwrap();
    assert!(result.conflicted);

    // Before running the resolver, capture the conflicted worktree content.
    let conflicted_before = std::fs::read_to_string(repo.join("a.txt")).unwrap();
    assert!(
        conflicted_before.contains("<<<<<<<"),
        "fixture must contain markers"
    );

    let backups_dir = tmp.path().join("backups");
    std::env::set_var("GC_BACKUP_DIR", &backups_dir);

    let report =
        auto_resolve_merge(&local_target(&repo), &rule_based_opts(), ai_disabled()).unwrap();
    assert!(report.committed);

    // The backup holds the conflicted original.
    let backups = git_companion::git::auto::list_backups(&local_target(&repo)).unwrap();
    assert_eq!(backups.len(), 1);
    let id = backups[0].id.clone();
    let raw = backups_dir.join(backup_slug(&repo)).join(&id).join("a.txt");
    assert_eq!(
        std::fs::read_to_string(&raw).unwrap(),
        conflicted_before,
        "backup must preserve the exact conflicted content"
    );

    git_companion::git::auto::restore_backup(&local_target(&repo), &id).unwrap();
    let restored = std::fs::read_to_string(repo.join("a.txt")).unwrap();
    assert_eq!(
        restored, conflicted_before,
        "restore must recover the original"
    );
}
