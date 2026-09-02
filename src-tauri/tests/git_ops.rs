//! Tests for git ops: add, commit, status.
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn git_run(dir: &Path, args: &[&str]) -> std::process::Output {
    let mut c = std::process::Command::new("git");
    c.args(args)
        .current_dir(dir)
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8");
    c.output().unwrap()
}

fn init_repo(dir: &Path) {
    git_run(dir, &["init", "-q"]);
    git_run(dir, &["config", "user.email", "test@x"]);
    git_run(dir, &["config", "user.name", "tester"]);
    git_run(dir, &["config", "commit.gpgsign", "false"]);
}

fn touch(path: &str) {
    let p = Path::new(path);
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(path, "contents").unwrap();
}

#[test]
fn add_stages_files() {
    let td = TempDir::new().unwrap();
    init_repo(td.path());
    // Create a file and stage it.
    touch(&format!("{}/new.txt", td.path().display()));
    let target = git_companion::git::Target::Local(td.path().into());
    git_companion::git::add(&target, &[format!("{}/new.txt", td.path().display())]).unwrap();
    // Status should show it as staged (index has change, worktree has change after touch).
    let status = git_companion::git::list_status(&target).unwrap();
    let staged: Vec<_> = status.files.iter().filter(|f| f.staged).collect();
    assert!(
        !staged.is_empty(),
        "expected some staged files after git add"
    );
}

#[test]
fn commit_with_message() {
    let td = TempDir::new().unwrap();
    init_repo(td.path());
    // Make an initial commit so the repo exists.
    touch(&format!("{}/a.txt", td.path().display()));
    git_run(td.path(), &["add", "-A"]);
    git_run(td.path(), &["commit", "-m", "initial"]);

    // Make a new change and commit it.
    touch(&format!("{}/b.txt", td.path().display()));
    let target = git_companion::git::Target::Local(td.path().into());
    let result = git_companion::git::commit(&target, "test commit", true).unwrap();
    assert!(result.ok, "commit should succeed: {}", result.message);
    assert!(result.sha.is_some(), "sha should be present");
}

#[test]
fn status_returns_branch_and_files() {
    let td = TempDir::new().unwrap();
    init_repo(td.path());
    git_run(td.path(), &["commit", "--allow-empty", "-m", "init"]);

    let target = git_companion::git::Target::Local(td.path().into());
    let status = git_companion::git::list_status(&target).unwrap();
    assert!(status.branch.is_some(), "branch should be present");
    assert_eq!(
        status.files.len(),
        0,
        "clean repo should have no changed files"
    );
}

// ── Merge center tests ─────────────────────────────────────────────────────────

use git_companion::git::merge::{
    abort_merge, complete_merge, conflict_detail, list_pending_branches, merge_in_progress,
    remaining_conflicts, resolve_conflict, start_merge, Resolution,
};
use git_companion::git::push;
use git_companion::git::Target;

fn make_bare_origin() -> (TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git_run(bare.path(), &["init", "--bare", "-q", "-b", "main"]);
    git_run(work.path(), &["init", "-q", "-b", "main"]);
    git_run(work.path(), &["config", "user.email", "test@x"]);
    git_run(work.path(), &["config", "user.name", "tester"]);
    git_run(work.path(), &["config", "commit.gpgsign", "false"]);
    git_run(
        bare.path(),
        &["config", "receive.denyCurrentBranch", "ignore"],
    );
    (bare, work)
}

fn add_origin_clone(work: &Path, bare: &Path) {
    let url = format!("file://{}", bare.display());
    git_run(work, &["remote", "add", "origin", &url]);
    git_run(work, &["push", "-q", "origin", "main"]);
    git_run(work, &["fetch", "-q", "origin"]);
}

fn seed_commit(work: &Path, file: &str, body: &str, msg: &str) {
    let p = format!("{}/{}", work.display(), file);
    fs::write(&p, body).unwrap();
    git_run(work, &["add", "-A"]);
    git_run(work, &["commit", "-q", "-m", msg]);
}

#[test]
fn pending_branches_list_ahead_and_changed_files() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/x"]);
    seed_commit(work.path(), "app.txt", "v1-x\n", "feat x");
    seed_commit(work.path(), "x.txt", "x\n", "feat x file");
    git_run(work.path(), &["push", "-q", "origin", "feature/x"]);
    git_run(work.path(), &["checkout", "-q", "main"]);

    let target = Target::Local(work.path().into());
    let pending = list_pending_branches(&target, "origin", "main").unwrap();
    let x = pending
        .iter()
        .find(|b| b.short_name == "feature/x")
        .expect("x branch listed");
    assert_eq!(x.ahead, 2);
    let paths: Vec<&str> = x.changed_files.iter().map(|c| c.path.as_str()).collect();
    assert!(paths.contains(&"app.txt"));
    assert!(paths.contains(&"x.txt"));
}

#[test]
fn pending_branches_excludes_merged_and_head() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/z"]);
    seed_commit(work.path(), "z.txt", "z\n", "feat z");
    git_run(work.path(), &["push", "-q", "origin", "feature/z"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    git_run(work.path(), &["merge", "--no-ff", "-q", "feature/z"]);
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["fetch", "-q", "origin", "--prune"]);

    let target = Target::Local(work.path().into());
    let pending = list_pending_branches(&target, "origin", "main").unwrap();
    assert!(pending.iter().all(|b| b.short_name != "feature/z"));
}

#[test]
fn merge_success_creates_no_ff_merge_commit() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/o"]);
    seed_commit(work.path(), "o.txt", "o\n", "feat o");
    git_run(work.path(), &["push", "-q", "origin", "feature/o"]);
    git_run(work.path(), &["checkout", "-q", "main"]);

    let target = Target::Local(work.path().into());
    let outcome = start_merge(&target, "origin/feature/o", "main", "origin").unwrap();
    assert!(outcome.ok);
    assert!(!outcome.conflicted);
    let out = git_run(work.path(), &["log", "-1", "--pretty=%P"]);
    let parents = String::from_utf8_lossy(&out.stdout);
    let pcount = parents.split_whitespace().count();
    assert_eq!(
        pcount, 2,
        "merge commit should have 2 parents, got: {parents}"
    );
    assert!(!merge_in_progress(&target).unwrap());
}

#[test]
fn merge_conflict_reports_files_and_keeps_merging_state() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "line1\nline2\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/a"]);
    seed_commit(work.path(), "app.txt", "line1-a\nline2\n", "feat a");
    git_run(work.path(), &["push", "-q", "origin", "feature/a"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    seed_commit(work.path(), "app.txt", "line1-b\nline2\n", "feat b");
    git_run(work.path(), &["push", "-q", "origin", "main"]);

    let target = Target::Local(work.path().into());
    let outcome = start_merge(&target, "origin/feature/a", "main", "origin").unwrap();
    assert!(outcome.conflicted);
    assert!(outcome.conflicted_files.iter().any(|p| p == "app.txt"));
    assert!(merge_in_progress(&target).unwrap());
}

#[test]
fn merge_rejected_when_worktree_dirty() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/d"]);
    seed_commit(work.path(), "d.txt", "d\n", "feat d");
    git_run(work.path(), &["push", "-q", "origin", "feature/d"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    let p = format!("{}/app.txt", work.path().display());
    fs::write(&p, "dirty\n").unwrap();
    git_run(work.path(), &["add", "-A"]);

    let target = Target::Local(work.path().into());
    let err = start_merge(&target, "origin/feature/d", "main", "origin").unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("변경"),
        "expected dirty-tree error, got: {msg}"
    );
    assert!(!merge_in_progress(&target).unwrap());
}

#[test]
fn abort_merge_restores_clean_tree() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "line1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/ab"]);
    seed_commit(work.path(), "app.txt", "line1-ab\n", "feat ab");
    git_run(work.path(), &["push", "-q", "origin", "feature/ab"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    seed_commit(work.path(), "app.txt", "line1-main\n", "main edit");

    let target = Target::Local(work.path().into());
    let outcome = start_merge(&target, "origin/feature/ab", "main", "origin").unwrap();
    assert!(outcome.conflicted);
    abort_merge(&target).unwrap();
    assert!(!merge_in_progress(&target).unwrap());
    let remaining = remaining_conflicts(&target).unwrap();
    assert!(remaining.is_empty());
    let status = git_run(work.path(), &["status", "--porcelain=v2"]);
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        !stdout.contains("U "),
        "tree should not have unmerged after abort: {stdout}"
    );
}

#[test]
fn resolve_manual_then_complete_merges_both_edits() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "line1\nline2\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/m"]);
    seed_commit(work.path(), "app.txt", "line1-m\nline2\n", "feat m");
    git_run(work.path(), &["push", "-q", "origin", "feature/m"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    seed_commit(work.path(), "app.txt", "line1-n\nline2\n", "feat n");

    let target = Target::Local(work.path().into());
    let _ = start_merge(&target, "origin/feature/m", "main", "origin").unwrap();
    assert!(merge_in_progress(&target).unwrap());
    let detail = conflict_detail(&target, "app.txt").unwrap();
    assert!(!detail.is_binary);
    assert!(!detail.too_large);
    // ours = current branch (main = line1-n), theirs = incoming branch (feature/m = line1-m).
    assert!(detail.ours.contains("line1-n"));
    assert!(detail.theirs.contains("line1-m"));
    let remaining = resolve_conflict(
        &target,
        "app.txt",
        &Resolution::Manual {
            content: "line1-merged\nline2\n".into(),
        },
    )
    .unwrap();
    assert!(
        remaining.is_empty(),
        "remaining should be empty, got {remaining:?}"
    );
    let outcome = complete_merge(&target, Some("feature/m 브랜치 병합")).unwrap();
    assert!(outcome.ok);
    assert!(!merge_in_progress(&target).unwrap());
    let show = git_run(work.path(), &["show", "HEAD:app.txt"]);
    let body = String::from_utf8_lossy(&show.stdout);
    assert!(body.contains("line1-merged"));
}

#[test]
fn conflict_detail_stages_with_missing_base_on_add_add() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "shared.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/add"]);
    seed_commit(work.path(), "new.txt", "from-add\n", "adds new.txt");
    git_run(work.path(), &["push", "-q", "origin", "feature/add"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    seed_commit(
        work.path(),
        "new.txt",
        "from-main\n",
        "main also adds new.txt",
    );

    let target = Target::Local(work.path().into());
    let outcome = start_merge(&target, "origin/feature/add", "main", "origin").unwrap();
    assert!(outcome.conflicted);
    let detail = conflict_detail(&target, "new.txt").unwrap();
    assert!(
        detail.base.is_none(),
        "add/add must yield base=None, got {:?}",
        detail.base
    );
    assert!(detail.ours.contains("from-main"));
    assert!(detail.theirs.contains("from-add"));
}

#[test]
fn push_branch_override_pushes_head_to_named_branch() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/p"]);
    seed_commit(work.path(), "p.txt", "p\n", "feat p");

    let target = Target::Local(work.path().into());
    let outcome = push(&target, Some("main"), None).unwrap();
    assert!(outcome.ok, "push outcome not ok: {}", outcome.message);
    let show = git_run(bare.path(), &["show", "main:p.txt"]);
    assert!(show.status.success(), "p.txt should be on remote main");
}

#[test]
fn pull_merges_divergent_and_reports_conflicts() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "base\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);

    // Second clone diverges on the same file.
    let other = TempDir::new().unwrap();
    let url = format!("file://{}", bare.path().display());
    git_run(other.path(), &["clone", "-q", &url, "clone"]);
    let other_work = other.path().join("clone");
    git_run(&other_work, &["config", "user.email", "test@x"]);
    git_run(&other_work, &["config", "user.name", "tester"]);
    git_run(&other_work, &["config", "commit.gpgsign", "false"]);
    let other_target = Target::Local(other_work.clone());
    seed_commit(&other_work, "app.txt", "local edit\n", "local work");

    // The first repo pushes a conflicting edit.
    seed_commit(work.path(), "app.txt", "remote edit\n", "remote work");
    git_run(work.path(), &["push", "-q", "origin", "main"]);

    // Pull in the divergent clone: must merge (not --ff-only), leave MERGE_HEAD
    // in place and report the conflicted file so the resolver UI can take over.
    let outcome = git_companion::git::pull(&other_target).unwrap();
    assert!(!outcome.ok, "pull must not fast-forward a divergent branch");
    assert_eq!(
        outcome.conflicted_files,
        vec!["app.txt".to_string()],
        "conflict should be reported: {}",
        outcome.message
    );
    assert!(
        git_companion::git::merge::merge_in_progress(&other_target).unwrap(),
        "MERGE_HEAD should remain for the resolver UI"
    );
    let remaining = git_companion::git::merge::remaining_conflicts(&other_target).unwrap();
    assert_eq!(remaining, vec!["app.txt".to_string()]);

    // Resolve and complete through the same path the UI uses.
    git_companion::git::merge::resolve_conflict(
        &other_target,
        "app.txt",
        &git_companion::git::merge::Resolution::Theirs,
    )
    .unwrap();
    let done = git_companion::git::merge::complete_merge(&other_target, Some("main 병합")).unwrap();
    assert!(done.ok, "merge completion should succeed: {}", done.message);
}

#[test]
fn pull_fast_forwards_when_no_divergence() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    seed_commit(work.path(), "app.txt", "v2\n", "second");
    git_run(work.path(), &["push", "-q", "origin", "main"]);

    let target = Target::Local(work.path().into());
    let outcome = git_companion::git::pull(&target).unwrap();
    assert!(outcome.ok, "pull should fast-forward: {}", outcome.message);
    assert!(outcome.conflicted_files.is_empty());
}

#[test]
fn pending_branches_lists_local_unpushed_branch_and_dedupes() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "wip/feature"]);
    seed_commit(work.path(), "w.txt", "w\n", "wip commit");
    // Not pushed — the merge center must still offer it as mergeable.

    let target = Target::Local(work.path().into());
    let pending = list_pending_branches(&target, "origin", "main").unwrap();
    let w = pending
        .iter()
        .find(|b| b.short_name == "wip/feature")
        .expect("local unpushed branch listed");
    assert!(w.local, "local flag set");
    assert_eq!(w.ahead, 1);
    assert!(
        pending.iter().all(|b| b.short_name != "main"),
        "base branch itself never listed"
    );

    // Once pushed, the entry switches to the remote form (dedup by sha).
    git_run(work.path(), &["push", "-q", "origin", "wip/feature"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    let pending = list_pending_branches(&target, "origin", "main").unwrap();
    let matching: Vec<_> = pending
        .iter()
        .filter(|b| b.short_name == "wip/feature")
        .collect();
    assert_eq!(matching.len(), 1, "local + remote same sha -> single entry");
    assert!(!matching[0].local);

    // And merging by the (now remote) ref works end to end.
    let out = start_merge(&target, "origin/wip/feature", "main", "origin").unwrap();
    assert!(out.ok, "merge of pending branch succeeds");
}

#[test]
fn pending_branches_skips_local_when_already_in_base() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "done/local"]);
    seed_commit(work.path(), "d.txt", "d\n", "local work");
    git_run(work.path(), &["checkout", "-q", "main"]);
    git_run(work.path(), &["merge", "--no-ff", "-q", "done/local"]);
    git_run(work.path(), &["push", "-q", "origin", "main"]);

    let target = Target::Local(work.path().into());
    let pending = list_pending_branches(&target, "origin", "main").unwrap();
    assert!(
        pending.iter().all(|b| b.short_name != "done/local"),
        "already-merged local branch skipped"
    );
}

// 원격 트래킹 이름(origin/…)으로 checkout_branch를 호출해도 로컬 브랜치로 정규화되어
// 전환되고, `origin/origin/…` 폴백 실패(파일명 버그)가 나지 않아야 한다.
#[test]
fn checkout_branch_normalizes_remote_prefixed_name() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    // 원격에만 존재하는 브랜치 (로컬에선 삭제, 원격 트래킹 ref만 남김)
    git_run(work.path(), &["checkout", "-q", "-b", "feature/rr"]);
    seed_commit(work.path(), "r.txt", "r\n", "remote-only work");
    git_run(work.path(), &["push", "-q", "origin", "feature/rr"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    git_run(work.path(), &["branch", "-q", "-D", "feature/rr"]);
    git_run(work.path(), &["fetch", "-q", "origin"]);

    let target = Target::Local(work.path().into());
    git_companion::git::checkout_branch(&target, "origin/feature/rr").unwrap();
    let cur = git_run(work.path(), &["branch", "--show-current"]);
    assert_eq!(String::from_utf8_lossy(&cur.stdout).trim(), "feature/rr");
    let local = git_run(
        work.path(),
        &["for-each-ref", "refs/heads", "--format=%(refname:short)"],
    );
    let heads = String::from_utf8_lossy(&local.stdout);
    assert!(
        heads.contains("feature/rr"),
        "local tracking branch should be created"
    );
}

// 작업 트리가 더럽고 대상 브랜치와 파일이 겹치면, `origin/origin/…` 같은 원시 에러 대신
// 한글로 된 안내 메시지가 나와야 한다.
#[test]
fn checkout_branch_dirty_tree_returns_friendly_errors() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/zz"]);
    seed_commit(work.path(), "app.txt", "v1-zz\n", "conflict with app.txt");
    git_run(work.path(), &["push", "-q", "origin", "feature/zz"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    git_run(work.path(), &["fetch", "-q", "origin"]);
    // 같은 파일(app.txt)을 손대면 전환 불가 상태가 된다.
    fs::write(work.path().join("app.txt"), "local uncommitted edit\n").unwrap();

    let target = Target::Local(work.path().into());
    let err = git_companion::git::checkout_branch(&target, "feature/zz").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("커밋되지 않은 변경사항"),
        "expected Korean friendly message, got: {msg}"
    );
    // 원격 접두가 붙은 이름으로 불러도 동일하게 친절한 메시지가 나와야 한다.
    let err2 = git_companion::git::checkout_branch(&target, "origin/feature/zz").unwrap_err();
    assert!(err2.to_string().contains("커밋되지 않은 변경사항"));
}

#[test]
fn stash_save_list_pop_and_drop_by_index() {
    use git_companion::git::ops::{list_stashes, StashAction};
    use git_companion::git::stash;

    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    // Dirty the tree (tracked file), stash it.
    seed_commit(work.path(), "other.txt", "o\n", "base");
    fs::write(format!("{}/other.txt", work.path().display()), "dirty\n").unwrap();
    stash(
        &Target::Local(work.path().into()),
        StashAction::Save {
            message: Some("임시 작업".into()),
        },
    )
    .unwrap();
    let target = Target::Local(work.path().into());
    let entries = list_stashes(&target).unwrap();
    assert_eq!(entries.len(), 1, "one stash should exist");
    assert_eq!(entries[0].index, "stash@{0}");
    assert!(
        entries[0].subject.contains("임시 작업"),
        "subject: {}",
        entries[0].subject
    );
    // Clean tree after stash.
    assert!(
        !fs::read_to_string(format!("{}/other.txt", work.path().display()))
            .unwrap()
            .contains("dirty")
    );

    // Pop by index restores the file and empties the stash.
    stash(&target, StashAction::PopIndex("stash@{0}".into())).unwrap();
    assert!(
        fs::read_to_string(format!("{}/other.txt", work.path().display()))
            .unwrap()
            .contains("dirty")
    );
    assert!(list_stashes(&target).unwrap().is_empty());

    // Drop by index works too.
    fs::write(format!("{}/other.txt", work.path().display()), "again\n").unwrap();
    stash(&target, StashAction::Save { message: None }).unwrap();
    stash(&target, StashAction::DropIndex("stash@{0}".into())).unwrap();
    assert!(list_stashes(&target).unwrap().is_empty());
}

#[test]
fn config_v6_default_ai_disabled() {
    let v5 = r#"{
        "schema_version": 5,
        "repositories": [],
        "projects": [],
        "external_tools": [],
        "ssh_profile": {
            "default_user": "",
            "default_key_path": "",
            "default_host": "",
            "connect_timeout": "5",
            "default_port": 22
        },
        "peer": {
            "backend_url": "",
            "device_token": ""
        }
    }"#;
    let mut cfg: git_companion::config_store::AppSettings = serde_json::from_str(v5).unwrap();
    assert!(
        !cfg.ai.enabled,
        "ai must default to disabled when v5 lacks the field"
    );
    assert_eq!(cfg.schema_version, 5);
    git_companion::config_store::migrate(&mut cfg).unwrap();
    assert_eq!(
        cfg.schema_version,
        git_companion::config_store::CURRENT_SCHEMA
    );
    assert!(!cfg.ai.enabled);
}

// ── SSH target (gated on GC_SSH_TEST_* env; skips gracefully in CI) ────────

fn ssh_sh(host: &str, user: &str, key: &str, port: u16, remote: &str) -> std::process::Output {
    let mut c = std::process::Command::new("ssh");
    c.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ConnectTimeout=5",
    ]);
    if !key.is_empty() {
        c.arg("-i").arg(key);
    }
    if port != 22 {
        c.arg("-p").arg(port.to_string());
    }
    c.arg(if user.is_empty() {
        host.to_string()
    } else {
        format!("{user}@{host}")
    });
    c.arg("--").arg(remote);
    c.output().unwrap()
}

#[test]
fn ssh_target_commit_with_spaces_and_quotes() {
    let host = std::env::var("GC_SSH_TEST_HOST").unwrap_or_default();
    if host.is_empty() {
        eprintln!("skipped: set GC_SSH_TEST_HOST (plus USER/KEY/PORT) to run");
        return;
    }
    let user = std::env::var("GC_SSH_TEST_USER").unwrap_or_default();
    let key = std::env::var("GC_SSH_TEST_KEY").unwrap_or_default();
    let port: u16 = std::env::var("GC_SSH_TEST_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(22);

    // Fresh repo on the remote (same machine in the test environment, but
    // addressed purely over SSH so arg/path quoting is exercised).
    let remote_repo = "/tmp/gc_ssh_test_repo";
    let setup = ssh_sh(
        &host,
        &user,
        &key,
        port,
        &format!(
            "rm -rf {remote_repo} && git init -q {remote_repo} && \
             git -C {remote_repo} config user.email test@x && \
             git -C {remote_repo} config user.name tester && \
             printf 'hi' > {remote_repo}/init.txt && \
             git -C {remote_repo} add -A && git -C {remote_repo} commit -qm init"
        ),
    );
    assert!(setup.status.success());

    let target = git_companion::git::Target::Ssh {
        user: user.clone(),
        host: host.clone(),
        key: key.clone(),
        password: String::new(),
        port,
        path: remote_repo.into(),
    };

    // 1. File inside a directory with spaces, committed with a multi-word
    //    message containing quotes (old argv-passing broke both).
    let sp = format!("{remote_repo}/a dir/note.txt");
    let sh = ssh_sh(
        &host,
        &user,
        &key,
        port,
        &format!(
            "mkdir -p '{remote_repo}/a dir' && printf 'hello' > '{}'",
            sp.replace('\'', "'\\''")
        ),
    );
    assert!(sh.status.success(), "remote file write failed");
    let add = git_companion::git::add(&target, &[sp.clone()]);
    assert!(
        add.is_ok(),
        "add with spaces path over ssh: {:?}",
        add.err()
    );
    let c1 = git_companion::git::commit(&target, "feat: first \"commit\"", false).unwrap();
    assert!(c1.ok, "commit over ssh: {}", c1.message);

    // 2. write_file / read_file round-trip over ssh (conflict-resolver path).
    git_companion::git::write_file_at_target(&target, "a dir/note.txt", b"updated body").unwrap();
    let body = git_companion::git::read_file_at_target(&target, "a dir/note.txt").unwrap();
    assert_eq!(body, b"updated body");
}

#[test]
fn ssh_password_auth_runs_git_and_files() {
    use git_companion::commands::repo::{browse_ssh_dir, SshTarget};
    use git_companion::git::Target;
    let host = std::env::var("GC_SSH_TEST_HOST").unwrap_or_default();
    let password = std::env::var("GC_SSH_TEST_PASSWORD").unwrap_or_default();
    if host.is_empty() || password.is_empty() {
        eprintln!("skipped: set GC_SSH_TEST_HOST/PASSWORD (plus PW_USER/PORT) to run");
        return;
    }
    let user = std::env::var("GC_SSH_TEST_PW_USER").unwrap_or_else(|_| "gctest".to_string());
    let port: u16 = std::env::var("GC_SSH_TEST_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(22);

    // Setup via sshpass (no app APIs involved here).
    let remote_repo = "/tmp/gc_ssh_pw_repo";
    let mut setup = std::process::Command::new("sshpass");
    setup
        .arg("-e")
        .arg("ssh")
        .arg("-o")
        .arg("StrictHostKeyChecking=yes")
        .arg("-o")
        .arg("NumberOfPasswordPrompts=1");
    if port != 22 {
        setup.arg("-p").arg(port.to_string());
    }
    setup
        .arg(format!("{user}@{host}"))
        .arg("--")
        .arg(format!(
            "rm -rf {remote_repo} && mkdir -p '{remote_repo}/sp ace' && git init -q {remote_repo} &&              git -C {remote_repo} config user.email t@t && git -C {remote_repo} config user.name t &&              printf a > '{remote_repo}/sp ace/f.txt' && git -C {remote_repo} add -A &&              git -C {remote_repo} commit -qm init"
        ))
        .env("SSHPASS", &password);
    let out = setup.output().unwrap();
    assert!(
        out.status.success(),
        "setup failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // App path #1: browse_ssh_dir with password auth.
    let target = SshTarget {
        ssh_user: user.clone(),
        ssh_host: host.clone(),
        ssh_key_path: String::new(),
        ssh_password: password.clone(),
        ssh_port: port,
    };
    let listing = browse_ssh_dir(target, remote_repo.to_string()).unwrap();
    assert!(
        listing.git_repo,
        "browse over password auth must see a work tree"
    );

    // App path #2: Target::from_repo → run_at_target / write / read.
    let t = Target::from_repo(remote_repo, &host, &user, "", &password, port);
    let ok = git_companion::git::run_at_target(&t, ["rev-parse", "--is-inside-work-tree"]).unwrap();
    assert_eq!(ok.stdout.trim(), "true");

    git_companion::git::write_file_at_target(&t, "sp ace/g.txt", b"hello password auth").unwrap();
    let read_back = git_companion::git::read_file_at_target(&t, "sp ace/g.txt").unwrap();
    assert_eq!(String::from_utf8_lossy(&read_back), "hello password auth");

    let add = git_companion::git::run_at_target(&t, ["add", "--", "sp ace/g.txt"]).unwrap();
    assert_eq!(add.status, 0, "add failed: {}", add.stderr);
    let cm = git_companion::git::run_at_target(
        &t,
        [
            "commit",
            "-m",
            "multi-word commit over password auth",
            "--allow-empty",
        ],
    )
    .unwrap();
    assert_eq!(cm.status, 0, "commit failed: {}", cm.stderr);
    assert!(cm.stdout.contains("multi-word commit over password auth"));
}

#[test]
/// Both a key and a (deliberately wrong) password: the password attempt is
/// rejected — the standard test recipe uses `PermitRootLogin
/// prohibit-password`, which refuses root password logins like Ubuntu's
/// default — and the app must automatically fall back to the key.
fn ssh_password_rejected_falls_back_to_key() {
    use git_companion::commands::repo::{browse_ssh_dir, SshTarget};
    use git_companion::git::Target;
    let host = std::env::var("GC_SSH_TEST_HOST").unwrap_or_default();
    let user = std::env::var("GC_SSH_TEST_USER").unwrap_or_default();
    let key = std::env::var("GC_SSH_TEST_KEY").unwrap_or_default();
    if host.is_empty() || user.is_empty() || key.is_empty() {
        eprintln!("skipped: set GC_SSH_TEST_HOST/USER/KEY to run");
        return;
    }
    let port: u16 = std::env::var("GC_SSH_TEST_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(22);

    let wrong_password = "definitely-wrong-password".to_string();

    // App path #1: browse_ssh_dir — password is rejected, key fallback must
    // produce a successful listing.
    let target = SshTarget {
        ssh_user: user.clone(),
        ssh_host: host.clone(),
        ssh_key_path: key.clone(),
        ssh_password: wrong_password.clone(),
        ssh_port: port,
    };
    let listing = browse_ssh_dir(target, "/tmp".to_string()).unwrap();
    assert!(
        !listing.entries.is_empty(),
        "key fallback must list /tmp entries"
    );

    // App path #2: git ops over Target::Ssh — auth must succeed via the key;
    // a not-a-repo path then yields git's 128, not ssh's 255.
    let t = Target::from_repo("/etc", &host, &user, &key, &wrong_password, port);
    let ok = git_companion::git::run_at_target(&t, ["rev-parse", "--is-inside-work-tree"]).unwrap();
    assert_eq!(
        ok.status, 128,
        "expected git's not-a-repo status (auth succeeded), got {} with stderr: {}",
        ok.status, ok.stderr
    );
}

#[test]
fn browse_ssh_dir_lists_remote_with_git_flag() {
    use git_companion::commands::repo::{browse_ssh_dir, SshTarget};
    let host = std::env::var("GC_SSH_TEST_HOST").unwrap_or_default();
    if host.is_empty() {
        eprintln!("skipped: set GC_SSH_TEST_HOST (plus USER/KEY/PORT) to run");
        return;
    }
    let user = std::env::var("GC_SSH_TEST_USER").unwrap_or_default();
    let key = std::env::var("GC_SSH_TEST_KEY").unwrap_or_default();
    let port: u16 = std::env::var("GC_SSH_TEST_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(22);

    let remote_repo = "/tmp/gc_ssh_browse_repo";
    let setup = ssh_sh(
        &host,
        &user,
        &key,
        port,
        &format!(
            "rm -rf {remote_repo} && mkdir -p '{remote_repo}/sub dir' && git init -q {remote_repo} && \
             printf 'a' > '{remote_repo}/a.txt' && printf 'b' > '{remote_repo}/sub dir/b.txt' && \
             printf 'x' > '{remote_repo}/.hidden'"
        ),
    );
    assert!(setup.status.success());

    let target = SshTarget {
        ssh_user: user,
        ssh_host: host,
        ssh_key_path: key,
        ssh_password: String::new(),
        ssh_port: port,
    };

    // Home (empty path) resolves to an absolute dir.
    let home = browse_ssh_dir(target.clone(), String::new()).unwrap();
    assert!(!home.path.is_empty());
    assert!(!home.git_repo);

    // The repo root: git flag on, entries include hidden + dirs + files.
    let listing = browse_ssh_dir(target.clone(), remote_repo.to_string()).unwrap();
    assert!(listing.git_repo, "path is inside a work tree");
    assert_eq!(listing.path, remote_repo);
    let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    for want in [".git", ".hidden", "a.txt", "sub dir"] {
        assert!(names.contains(&want), "missing {want} in {names:?}");
    }
    let sub = listing
        .entries
        .iter()
        .find(|e| e.name == "sub dir")
        .unwrap();
    assert!(sub.is_dir);

    // Subdir inside the work tree still reports git_repo.
    let sub_listing = browse_ssh_dir(target.clone(), format!("{remote_repo}/sub dir")).unwrap();
    assert!(sub_listing.git_repo);
    assert_eq!(sub_listing.entries.len(), 1);
    assert_eq!(sub_listing.entries[0].name, "b.txt");

    // Nonexistent path → error, not a crash.
    assert!(browse_ssh_dir(target, "/no/such/dir".to_string()).is_err());
}

// ── push credentials (.gpconfig-era) ───────────────────────────────────────────

// HTTPS 원격 + 자격증명 없음 → auth_required 푸시 아웃컴 (git 프롬프트 행 안 함).
#[test]
fn push_https_without_credentials_reports_auth_required() {
    let td = TempDir::new().unwrap();
    init_repo(td.path());
    touch(&format!("{}/a.txt", td.path().display()));
    git_run(td.path(), &["add", "-A"]);
    git_run(td.path(), &["commit", "-q", "-m", "init"]);
    git_run(
        td.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://example.com/team/repo.git",
        ],
    );

    let target = Target::Local(td.path().into());
    let outcome = git_companion::git::push(&target, None, None).unwrap();
    assert!(!outcome.ok);
    assert!(
        outcome.auth_required,
        "auth_required should be set for https without creds"
    );
    assert!(
        outcome.message.contains("로그인"),
        "message should guide login: {}",
        outcome.message
    );
}

// GIT_ASKPASS 스크립트가 git credential fill에 올바른 아이디/비밀번호를 준다 (오프라인).
#[test]
fn askpass_script_answers_git_credential_prompt() {
    use std::process::Command;
    let script = git_companion::git::ops::askpass_script("devuser", "s3cret!pw");
    let dir = TempDir::new().unwrap();
    let script_path = dir.path().join("ask.sh");
    std::fs::write(&script_path, &script).unwrap();
    // GIT_ASKPASS는 스크립트를 직접 실행하므로 실행 권한이 필요하다.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let input = "protocol=https\nhost=example.com\n\n";
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "printf '%b' \"$1\" | GIT_ASKPASS={} GIT_TERMINAL_PROMPT=0 git credential fill",
            script_path.display()
        ))
        .arg("sh")
        .arg(input)
        .env("LC_ALL", "C.UTF-8")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "credential fill failed: {stdout}"
    );
    assert!(
        stdout.contains("username=devuser"),
        "missing username in: {stdout}"
    );
    assert!(
        stdout.contains("password=s3cret!pw"),
        "missing password in: {stdout}"
    );
}

// HTTPS 자격증명을 넣으면 푸시가 askpass 경로로 진행된다 (연결 거부까지 도달 — 인증 전 단계).
#[test]
fn push_https_with_credentials_runs_askpass_path() {
    let td = TempDir::new().unwrap();
    init_repo(td.path());
    touch(&format!("{}/a.txt", td.path().display()));
    git_run(td.path(), &["add", "-A"]);
    git_run(td.path(), &["commit", "-q", "-m", "init"]);
    git_run(
        td.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://127.0.0.1:9/team/repo.git",
        ],
    );

    let target = Target::Local(td.path().into());
    let cred = git_companion::config_store::PushCredential {
        username: "devuser".into(),
        password: "pw".into(),
    };
    let outcome = git_companion::git::push(&target, None, Some(&cred)).unwrap();
    // 연결 거부까지 갔으므로 auth_required가 아니어야 하고, askpass 스크립트는 정리되어야 한다.
    assert!(!outcome.ok);
    assert!(!outcome.auth_required);
    assert!(
        std::fs::read_dir(std::env::temp_dir()).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("gc-askpass-")
        }),
        "askpass script should be cleaned up"
    );
}

// ── .gpconfig ──────────────────────────────────────────────────────────────────

use git_companion::gpconfig::{member_from_account, ProjectConfig};

#[test]
fn gpconfig_save_read_roundtrip_and_commit() {
    let td = TempDir::new().unwrap();
    init_repo(td.path());
    touch(&format!("{}/a.txt", td.path().display()));
    git_run(td.path(), &["add", "-A"]);
    git_run(td.path(), &["commit", "-q", "-m", "init"]);

    let target = Target::Local(td.path().into());
    let (cfg, exists) = git_companion::gpconfig::read_config(&target).unwrap();
    assert!(!exists, "no .gpconfig yet");
    assert!(cfg.members.is_empty());

    let mut cfg = ProjectConfig::default();
    cfg.default_base_branch = "main".into();
    cfg.members.push(member_from_account(
        "acc1",
        "홍길동",
        "hong@example.com",
        "admin",
    ));
    cfg.members.push(member_from_account(
        "acc2",
        "김철수",
        "kim@example.com",
        "member",
    ));
    cfg.merge_managers
        .insert("feature/x".into(), "hong@example.com".into());
    cfg.notify_recipients.push("kim@example.com".into());
    let saved = git_companion::gpconfig::save_config(&target, &cfg).unwrap();
    assert_eq!(saved.members.len(), 2);

    let (back, exists) = git_companion::gpconfig::read_config(&target).unwrap();
    assert!(exists);
    assert_eq!(back.members.len(), 2);
    assert_eq!(
        back.merge_managers.get("feature/x").map(String::as_str),
        Some("hong@example.com")
    );
    assert_eq!(back.notify_recipients, vec!["kim@example.com".to_string()]);
    assert_eq!(
        back.gpconfig_version,
        git_companion::gpconfig::GPCONFIG_VERSION
    );

    // 커밋하면 로그에 남는다 — 다른 참여자가 pull로 받아가는 전달 경로.
    let out = git_companion::gpconfig::commit_config(&target).unwrap();
    assert!(out.ok, "commit failed: {}", out.message);
    let log = git_run(td.path(), &["log", "-1", "--format=%s"]);
    assert_eq!(
        String::from_utf8_lossy(&log.stdout).trim(),
        "chore: update project config (.gpconfig)"
    );
}

#[test]
fn config_store_accounts_and_push_credentials_roundtrip() {
    // config_store는 실제 홈 config를 쓰므로, 설정 직렬화/역직렬화로 검증한다.
    let mut s = git_companion::config_store::AppSettings::default();
    s.accounts.push(git_companion::config_store::Account {
        id: uuid::Uuid::new_v4(),
        name: "홍길동".into(),
        email: "hong@example.com".into(),
        username: None,
        password_hash: None,
        created_at: chrono::Utc::now(),
    });
    s.active_account_id = Some(s.accounts[0].id.to_string());
    s.push_credentials.insert(
        "repo-1".into(),
        git_companion::config_store::PushCredential {
            username: "devuser".into(),
            password: "pw".into(),
        },
    );
    let json = serde_json::to_string(&s).unwrap();
    let back: git_companion::config_store::AppSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.accounts.len(), 1);
    assert_eq!(back.accounts[0].email, "hong@example.com");
    assert_eq!(
        back.active_account_id.as_deref(),
        Some(s.accounts[0].id.to_string().as_str())
    );
    assert_eq!(
        back.push_credentials.get("repo-1").unwrap().username,
        "devuser"
    );
}
