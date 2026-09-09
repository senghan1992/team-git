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
    abort_merge, complete_merge, conflict_detail, delete_remote_branch, list_merged_remote_branches,
    list_pending_branches, merge_in_progress, remaining_conflicts, resolve_conflict, start_merge,
    Resolution,
};
use git_companion::git::merge::{parse_pending_output, pending_probe_script};
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

/// 병합 커밋은 만들어졌지만 push가 실패/취소된 상태 — 대기 목록이 그 브랜치를
/// 다시 "병합하라"고 세우면 안 되고(merged_locally 플래그), base가 원격보다
/// 앞선 커밋 수(base_unpushed_count)로 push 배너를 재구성할 수 있어야 한다.
#[test]
fn merged_but_unpushed_branch_is_flagged_not_relisted() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/m"]);
    seed_commit(work.path(), "m.txt", "m\n", "feat m");
    git_run(work.path(), &["push", "-q", "origin", "feature/m"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    // 병합은 했지만 push는 하지 않았다 (push 실패/취소 시나리오).
    git_run(work.path(), &["merge", "--no-ff", "-q", "feature/m"]);

    let target = Target::Local(work.path().into());
    let pending = list_pending_branches(&target, "origin", "main").unwrap();
    let m = pending
        .iter()
        .find(|b| b.short_name == "feature/m")
        .expect("아직 원격 main에 없으므로 목록에는 남는다");
    assert!(
        m.merged_locally,
        "로컬 main에 이미 병합됐음을 표시해야 한다 — UI가 '푸시 대기'로 보여 준다"
    );

    let unpushed =
        git_companion::git::merge::base_unpushed_count(&target, "origin", "main").unwrap();
    assert_eq!(unpushed, 2, "병합 커밋 + feat 커밋이 원격보다 앞서 있다");

    // push하면 둘 다 사라진다.
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["fetch", "-q", "origin"]);
    let pending = list_pending_branches(&target, "origin", "main").unwrap();
    assert!(pending.iter().all(|b| b.short_name != "feature/m"));
    let unpushed =
        git_companion::git::merge::base_unpushed_count(&target, "origin", "main").unwrap();
    assert_eq!(unpushed, 0);
}

/// 병합이 끝난 원격 브랜치 정리 — 목록에 뜨고, 삭제되며, 아직 병합 안 된
/// 브랜치는 거부된다 (팀원의 커밋이 지워지면 안 된다).
#[test]
fn merged_remote_branches_are_listed_and_deletable() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);

    // done: 병합·push까지 끝난 브랜치. wip: 아직 병합 안 된 브랜치.
    git_run(work.path(), &["checkout", "-q", "-b", "feature/done"]);
    seed_commit(work.path(), "done.txt", "d\n", "feat done");
    git_run(work.path(), &["push", "-q", "origin", "feature/done"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    git_run(work.path(), &["merge", "--no-ff", "-q", "feature/done"]);
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/wip"]);
    seed_commit(work.path(), "wip.txt", "w\n", "feat wip");
    git_run(work.path(), &["push", "-q", "origin", "feature/wip"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    git_run(work.path(), &["fetch", "-q", "origin"]);

    let target = Target::Local(work.path().into());
    let merged = list_merged_remote_branches(&target, "origin", "main").unwrap();
    let names: Vec<&str> = merged.iter().map(|b| b.short_name.as_str()).collect();
    assert!(names.contains(&"feature/done"), "병합 끝난 브랜치는 목록에 (got {names:?})");
    assert!(!names.contains(&"feature/wip"), "병합 안 된 브랜치는 제외");
    assert!(!names.contains(&"main"), "base 자신은 제외");

    // 병합 안 된 브랜치 삭제는 거부.
    let err = delete_remote_branch(&target, "origin", "main", "feature/wip", None)
        .expect_err("병합 안 된 브랜치는 지울 수 없어야 한다");
    assert!(err.to_string().contains("없는 커밋"), "이유를 말한다: {err}");
    // base 삭제도 거부.
    assert!(delete_remote_branch(&target, "origin", "main", "main", None).is_err());

    // 병합 끝난 브랜치는 삭제되고, 원격 ref와 트래킹 ref 모두 사라진다.
    delete_remote_branch(&target, "origin", "main", "feature/done", None).unwrap();
    let ls = git_run(work.path(), &["ls-remote", "--heads", "origin", "feature/done"]);
    assert!(
        String::from_utf8_lossy(&ls.stdout).trim().is_empty(),
        "원격에서 지워져야 한다"
    );
    let merged = list_merged_remote_branches(&target, "origin", "main").unwrap();
    assert!(merged.iter().all(|b| b.short_name != "feature/done"));
    // 같은 이름의 로컬 브랜치도 함께 정리됐어야 한다 — "지웠는데 git branch
    // 에 그대로 보인다"가 없어야 한다.
    let local = git_run(
        work.path(),
        &["show-ref", "--verify", "-q", "refs/heads/feature/done"],
    );
    assert!(!local.status.success(), "로컬 브랜치도 정리돼야 한다");
}

/// 원격 브랜치 삭제 후 로컬 사본 정리 규칙:
/// - 같은 이름의 로컬 브랜치 커밋이 전부 base 에 들어갔으면 함께 지운다
///   (누가 `git branch` 를 쳐도 안 보인다).
/// - 로컬에 아직 base 에 없는 커밋이 남아 있으면 **지우지 않고** 유지하며
///   (커밋 유실 방지), 결과에 사유를 실어 UI 가 안내하게 한다.
#[test]
fn delete_remote_branch_cleans_local_copy_when_safe() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);

    // feature/merged: 병합·push 완료 → 로컬 브랜치도 base 의 조상 → 정리 대상.
    git_run(work.path(), &["checkout", "-q", "-b", "feature/merged"]);
    seed_commit(work.path(), "m.txt", "m\n", "feat merged");
    git_run(work.path(), &["push", "-q", "origin", "feature/merged"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    git_run(work.path(), &["merge", "--no-ff", "-q", "feature/merged"]);
    git_run(work.path(), &["push", "-q", "origin", "main"]);

    // feature/ahead: 원격 tip 은 병합됐지만 **로컬에만** 커밋이 하나 더 있다.
    git_run(work.path(), &["checkout", "-q", "-b", "feature/ahead"]);
    seed_commit(work.path(), "a.txt", "a\n", "feat ahead");
    git_run(work.path(), &["push", "-q", "origin", "feature/ahead"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    git_run(work.path(), &["merge", "--no-ff", "-q", "feature/ahead"]);
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "feature/ahead"]);
    seed_commit(work.path(), "a2.txt", "a2\n", "local only");
    git_run(work.path(), &["checkout", "-q", "main"]);
    git_run(work.path(), &["fetch", "-q", "origin"]);

    let target = Target::Local(work.path().into());

    // 1) 병합 끝난 브랜치 삭제 → 원격·트래킹·로컬 브랜치 모두 정리.
    let out = delete_remote_branch(&target, "origin", "main", "feature/merged", None).unwrap();
    assert!(out.ok, "{}", out.message);
    assert!(
        out.cleaned_locally.iter().any(|s| s.contains("feature/merged")),
        "로컬 브랜치 정리 사실을 알려야 한다: {:?}",
        out.cleaned_locally
    );
    let ls = git_run(work.path(), &["ls-remote", "--heads", "origin", "feature/merged"]);
    assert!(
        String::from_utf8_lossy(&ls.stdout).trim().is_empty(),
        "원격에서 지워져야 한다"
    );
    let local = git_run(
        work.path(),
        &["show-ref", "--verify", "-q", "refs/heads/feature/merged"],
    );
    assert!(!local.status.success(), "로컬 브랜치도 지워져야 한다");
    let tracking = git_run(
        work.path(),
        &["show-ref", "--verify", "-q", "refs/remotes/origin/feature/merged"],
    );
    assert!(!tracking.status.success(), "트래킹 ref 도 지워져야 한다");
    let branch = git_run(work.path(), &["branch"]);
    assert!(
        !String::from_utf8_lossy(&branch.stdout).contains("feature/merged"),
        "git branch 에 안 보여야 한다"
    );

    // 2) 로컬에만 커밋이 남은 브랜치 → 원격은 삭제되지만 로컬은 유지된다.
    let out2 = delete_remote_branch(&target, "origin", "main", "feature/ahead", None).unwrap();
    assert!(out2.ok, "{}", out2.message);
    assert!(
        out2.kept_locally.iter().any(|s| s.contains("feature/ahead")),
        "유지 사유를 알려야 한다: {:?}",
        out2.kept_locally
    );
    let local2 = git_run(
        work.path(),
        &["show-ref", "--verify", "-q", "refs/heads/feature/ahead"],
    );
    assert!(local2.status.success(), "커밋이 남은 로컬 브랜치는 유지");
    let log = git_run(work.path(), &["log", "--oneline", "feature/ahead"]);
    assert!(
        String::from_utf8_lossy(&log.stdout).contains("local only"),
        "커밋이 유실되면 안 된다"
    );
}

/// "본인이 만든 브랜치만 삭제" 판정 규칙 — 이름 또는 이메일이 현재 사용자와
/// 일치하면 내 브랜치다.
#[test]
fn identity_matches_own_branch_rules() {
    use git_companion::git::merge::identity_matches;
    // 이름이 같으면 내 브랜치 (이메일은 달라도).
    assert!(identity_matches("홍길동", "old@x.kr", Some("홍길동"), Some("new@x.kr")));
    // 이름은 달라도 이메일이 같으면 내 브랜치 — 팀원 동명이인 등.
    assert!(identity_matches("hong", "hong@team.kr", Some("홍길동"), Some("hong@team.kr")));
    // 공백·대소문자는 무시.
    assert!(identity_matches("  Hong ", "HONG@team.kr", Some("hong"), Some("hong@team.kr")));
    // 둘 다 다르면 남의 브랜치.
    assert!(!identity_matches("kim", "kim@team.kr", Some("hong"), Some("hong@team.kr")));
    // 신원을 모르면 어떤 브랜치도 내 것이 아니다 — 삭제를 허락하지 않는다.
    assert!(!identity_matches("hong", "hong@team.kr", None, None));
    // 작성자 정보가 비어 있으면 내 브랜치가 아니다.
    assert!(!identity_matches("", "", Some("hong"), Some("hong@team.kr")));
}

/// 현재 사용자 신원 — 로컬 저장소 git 설정 우선, 없으면 로그인 계정 대체.
#[test]
fn current_git_identity_prefers_repo_config_then_session() {
    use git_companion::git::merge::current_git_identity;

    // 1) git 설정이 있으면 그것을 쓴다 (세션은 무시).
    let td = TempDir::new().unwrap();
    init_repo(td.path()); // user.name=tester, user.email=test@x
    let target = Target::Local(td.path().into());
    let (n, e) = current_git_identity(&target, Some(("세션홍길동", "session@x.kr")));
    assert_eq!(n.as_deref(), Some("tester"));
    assert_eq!(e.as_deref(), Some("test@x"));

    // 2) git 설정이 없으면(로컬·글로벌 모두 비어 있음) 로그인 계정으로 대체.
    //    (머신에 글로벌 git 신원이 있어도 로컬 빈 값이 우선한다 — 빈 값으로
    //    덮으면 조회가 글로벌로 내려가지 않는다.)
    let td2 = TempDir::new().unwrap();
    git_run(td2.path(), &["init", "-q"]);
    git_run(td2.path(), &["config", "user.name", ""]);
    git_run(td2.path(), &["config", "user.email", ""]);
    let target2 = Target::Local(td2.path().into());
    let (n2, e2) = current_git_identity(&target2, Some(("홍길동", "hong@team.kr")));
    assert_eq!(n2.as_deref(), Some("홍길동"));
    assert_eq!(e2.as_deref(), Some("hong@team.kr"));

    // 3) 둘 다 없으면 None — 아무 브랜치도 내 것이 아니다.
    let (n3, e3) = current_git_identity(&target2, None);
    assert!(n3.is_none() && e3.is_none());
}

/// modify/delete 충돌 — 한쪽이 파일을 지우고 다른 쪽이 수정했다. 충돌 표시가
/// 없어서 블록 편집기는 빈 화면이 되는 케이스다. 삭제된 쪽을 고르면 파일이
/// 지워진 채로 스테이징되어야 한다.
#[test]
fn modify_delete_conflict_resolves_to_deletion() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    seed_commit(work.path(), "doomed.txt", "old\n", "add doomed");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/del"]);
    git_run(work.path(), &["rm", "-q", "doomed.txt"]);
    git_run(work.path(), &["commit", "-q", "-m", "delete doomed"]);
    git_run(work.path(), &["push", "-q", "origin", "feature/del"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    seed_commit(work.path(), "doomed.txt", "modified\n", "modify doomed");

    let target = Target::Local(work.path().into());
    let outcome = start_merge(&target, "origin/feature/del", "main", "origin", None).unwrap();
    assert!(outcome.conflicted, "modify/delete는 충돌이어야 한다");
    assert!(outcome.conflicted_files.contains(&"doomed.txt".to_string()));

    // theirs(삭제한 쪽)를 고른다 → checkout --theirs 는 스테이지가 없어
    // 실패하고, 삭제 반영으로 넘어가야 한다.
    let remaining = resolve_conflict(&target, "doomed.txt", &Resolution::Theirs).unwrap();
    assert!(remaining.is_empty(), "충돌이 남으면 안 된다: {remaining:?}");
    assert!(
        !work.path().join("doomed.txt").exists(),
        "삭제를 골랐으니 파일이 지워져야 한다"
    );
    let done = complete_merge(&target, None).unwrap();
    assert!(done.ok, "병합 커밋이 만들어져야 한다: {}", done.message);
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
    let outcome = start_merge(&target, "origin/feature/o", "main", "origin", None).unwrap();
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
    let outcome = start_merge(&target, "origin/feature/a", "main", "origin", None).unwrap();
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
    let err = start_merge(&target, "origin/feature/d", "main", "origin", None).unwrap_err();
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
    let outcome = start_merge(&target, "origin/feature/ab", "main", "origin", None).unwrap();
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
    let _ = start_merge(&target, "origin/feature/m", "main", "origin", None).unwrap();
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
    let outcome = start_merge(&target, "origin/feature/add", "main", "origin", None).unwrap();
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
    let out = start_merge(&target, "origin/wip/feature", "main", "origin", None).unwrap();
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
    let listing = tokio_test::block_on(browse_ssh_dir(target, remote_repo.to_string())).unwrap();
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
    let listing = tokio_test::block_on(browse_ssh_dir(target, "/tmp".to_string())).unwrap();
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
    let home = tokio_test::block_on(browse_ssh_dir(target.clone(), String::new())).unwrap();
    assert!(!home.path.is_empty());
    assert!(!home.git_repo);

    // The repo root: git flag on, entries include hidden + dirs + files.
    let listing = tokio_test::block_on(browse_ssh_dir(target.clone(), remote_repo.to_string())).unwrap();
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
    let sub_listing =
        tokio_test::block_on(browse_ssh_dir(target.clone(), format!("{remote_repo}/sub dir")))
            .unwrap();
    assert!(sub_listing.git_repo);
    assert_eq!(sub_listing.entries.len(), 1);
    assert_eq!(sub_listing.entries[0].name, "b.txt");

    // Nonexistent path → error, not a crash.
    assert!(tokio_test::block_on(browse_ssh_dir(target, "/no/such/dir".to_string())).is_err());
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

// HTTPS 자격증명 푸시 — Basic 인증을 실제로 요구하는 로컬 git http 서버에서
// **엔드투엔드로 성공**하는지 검증한다 (옛 askpass 방식은 Windows Git 에서
// 실패했으므로, 이 테스트가 대체 경로를 끝까지 확인한다).
#[test]
#[cfg(unix)]
fn push_https_with_credentials_succeeds_end_to_end() {
    // python3 없으면 (개발 머신 제약) 조용히 스킵한다.
    if !std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return;
    }

    // 1) bare 원격 저장소 + 작업 저장소 준비
    let td = TempDir::new().unwrap();
    let origin = td.path().join("origin.git");
    git_run(td.path(), &["init", "--bare", "-q", origin.to_str().unwrap()]);
    // git http-backend 는 receive-pack 이 기본 비활성 — 켠다.
    git_run(&origin, &["config", "http.receivepack", "true"]);
    git_run(&origin, &["config", "http.uploadpack", "true"]);

    let work = td.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    init_repo(&work);
    touch(&format!("{}/a.txt", work.display()));
    git_run(&work, &["add", "-A"]);
    git_run(&work, &["commit", "-q", "-m", "init commit"]);

    // 2) Basic 인증 git http 서버 (python http.server + git http-backend)
    let username = "alice";
    let password = "p@ss 'word/!:&%"; // 특수문자 투성이 — CLI 인증과 같은 환경
    let port = free_port();
    let script = td.path().join("githttp.py");
    std::fs::write(&script, http_basic_git_server_py()).unwrap();
    let mut server = std::process::Command::new("python3")
        .arg(&script)
        .arg(origin.parent().unwrap().to_str().unwrap())
        .arg(port.to_string())
        .arg(username)
        .arg(password)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("http server spawn");
    // 서버 기동 대기 — 인증 없이 401 이라도 응답만 오면 살아 있는 것이다.
    let url = format!("http://127.0.0.1:{port}/origin.git");
    let mut ready = false;
    for _ in 0..40 {
        if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            use std::io::Write;
            let _ = s.write_all(b"GET / HTTP/1.0\r\n\r\n");
            let mut buf = [0u8; 64];
            use std::io::Read;
            if let Ok(n) = s.read(&mut buf) {
                if n > 0 {
                    ready = true;
                    break;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    assert!(ready, "http server did not start");

    git_run(&work, &["remote", "add", "origin", &url]);
    let target = Target::Local(work.clone());

    // 3) 특수문자 자격증명으로 푸시 → 성공해야 한다
    let cred = git_companion::config_store::PushCredential {
        username: username.into(),
        password: password.into(),
    };
    let outcome = git_companion::git::push(&target, Some("main"), Some(&cred)).unwrap();
    assert!(outcome.ok, "push failed: {}", outcome.message);
    // 원격에 실제로 반영되었는지 bare 저장소에서 확인 (bare 의 HEAD 는 빈
    // master 를 가리키므로 가지 이름을 명시한다).
    let log = git_run(&origin, &["log", "main", "-1", "--format=%s"]);
    assert_eq!(
        String::from_utf8_lossy(&log.stdout).trim(),
        "init commit"
    );

    // 4) 틀린 비밀번호 → auth_required=true + 사유가 메시지에 남는다
    let bad = git_companion::config_store::PushCredential {
        username: "alice".into(),
        password: "wrong-pw".into(),
    };
    let outcome2 = git_companion::git::push(&target, Some("main"), Some(&bad)).unwrap();
    assert!(!outcome2.ok);
    assert!(outcome2.auth_required, "텍 틀린 자격증명은 auth_required");
    assert!(
        outcome2.message.contains("로그인 실패"),
        "사유가 보여야 한다: {}",
        outcome2.message
    );

    // 5) 원격 브랜치 삭제(`push --delete`)도 같은 인증 경로를 타야 한다 —
    //    병합이 끝난 브랜치를 만들고, 자격증명 없이/틀린 값으로는
    //    auth_required, 올바른 값으로는 실제로 삭제되는지 확인한다.
    git_run(&work, &["checkout", "-q", "-b", "feature/legacy"]);
    touch(&format!("{}/legacy.txt", work.display()));
    git_run(&work, &["add", "-A"]);
    git_run(&work, &["commit", "-q", "-m", "legacy work"]);
    let push_legacy =
        git_companion::git::push(&target, Some("feature/legacy"), Some(&cred)).unwrap();
    assert!(push_legacy.ok, "branch push failed: {}", push_legacy.message);
    // main 에 병합·푸시 → feature/legacy 는 origin/main 의 조상이 된다.
    git_run(&work, &["checkout", "-q", "main"]);
    git_run(&work, &["merge", "-q", "--no-ff", "feature/legacy", "-m", "merge legacy"]);
    let push_main = git_companion::git::push(&target, Some("main"), Some(&cred)).unwrap();
    assert!(push_main.ok, "main push failed: {}", push_main.message);

    // 5-1) 자격증명 없이 삭제 → 프롬프트에 매달리지 않고 auth_required.
    let no_creds =
        git_companion::git::merge::delete_remote_branch(&target, "origin", "main", "feature/legacy", None)
            .unwrap();
    assert!(!no_creds.ok && no_creds.auth_required, "자격증명 없이 HTTPS 삭제는 auth_required: {}", no_creds.message);
    assert!(no_creds.message.contains("로그인"), "로그인 안내가 필요하다: {}", no_creds.message);

    // 5-2) 틀린 비밀번호 → auth_required + 사유.
    let bad_del = git_companion::git::merge::delete_remote_branch(
        &target, "origin", "main", "feature/legacy", Some(&bad),
    )
    .unwrap();
    assert!(!bad_del.ok && bad_del.auth_required, "틀린 자격증명 삭제는 auth_required: {}", bad_del.message);

    // 5-3) 올바른 자격증명 → 삭제 성공, 원격 ref 도 사라진다.
    let del = git_companion::git::merge::delete_remote_branch(
        &target, "origin", "main", "feature/legacy", Some(&cred),
    )
    .unwrap();
    assert!(del.ok, "delete failed: {}", del.message);
    let ls = git_run(&origin, &["ls-remote", "--heads", "origin", "feature/legacy"]);
    assert!(
        String::from_utf8_lossy(&ls.stdout).trim().is_empty(),
        "원격 ref 가 남아 있다: {}",
        String::from_utf8_lossy(&ls.stdout)
    );
    assert!(
        git_run(&work, &["rev-parse", "-q", "--verify", "refs/remotes/origin/feature/legacy"])
            .status
            .code()
            != Some(0),
        "fetch --prune 후 트래킹 ref 도 정리돼야 한다"
    );

    // 6) 병합 요청(refs/gc-mr/*) 푸시도 같은 자격증명 경로를 타야 한다 —
    //    GitHub 식 흐름의 핵심이다. push만으로는 대기열에 오르지 않고,
    //    명시적으로 요청을 보내야 관리자 대기열에 보인다.
    git_run(&work, &["checkout", "-q", "-b", "feature/mr"]);
    touch(&format!("{}/mr.txt", work.display()));
    git_run(&work, &["add", "-A"]);
    git_run(&work, &["commit", "-q", "-m", "mr work"]);
    let push_mr = git_companion::git::push(&target, Some("feature/mr"), Some(&cred)).unwrap();
    assert!(push_mr.ok, "branch push failed: {}", push_mr.message);

    // 6-1) 자격증명 없이 요청 → 시도조차 안 하고 local_only (프롬프트 금지).
    let req_anon = git_companion::git::mr::request_merge(
        &target, "origin", "main", "feature/mr", Some("MR 테스트"), "민지", "m@x", None,
    )
    .unwrap();
    assert!(req_anon.local_only, "자격증명 없는 HTTPS 요청은 원격 공유 불가");
    let remote_refs = git_run(&origin, &["for-each-ref", "refs/gc-mr"]);
    assert!(
        String::from_utf8_lossy(&remote_refs.stdout).trim().is_empty(),
        "자격증명 없는 요청이 원격에 없어야 한다"
    );

    // 6-2) 틀린 자격증명 → 역시 원격 공유 실패.
    let req_bad = git_companion::git::mr::request_merge(
        &target, "origin", "main", "feature/mr", Some("MR 테스트"), "민지", "m@x", Some(&bad),
    )
    .unwrap();
    assert!(req_bad.local_only, "틀린 자격증명 요청도 local_only");

    // 6-3) 올바른 자격증명 → 원격에 실제로 올라간다.
    let req_ok = git_companion::git::mr::request_merge(
        &target, "origin", "main", "feature/mr", Some("MR 테스트"), "민지", "m@x", Some(&cred),
    )
    .unwrap();
    assert!(!req_ok.local_only, "올바른 자격증명 요청은 원격에 공유된다");

    // 6-4) 관리자(다른 폴더)가 clone + fetch --prune 하면 요청이 대기열에
    //      보인다 — 읽기까지 잠근 호스트 테스트를 위해 자격증명 fetch 를 쓴다.
    let manager = td.path().join("manager");
    let extra = format!(
        "AUTHORIZATION: Basic {}",
        git_companion::git::ops::base64_encode(format!("{username}:{password}").as_bytes())
    );
    let mut clone = std::process::Command::new("git");
    clone
        .args(["-c", &format!("http.extraheader={extra}")])
        .args(["clone", "-q", &url, manager.to_str().unwrap()])
        .current_dir(td.path())
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = clone.status().unwrap();
    assert!(status.success(), "자격증명 clone 이 성공해야 한다");
    let mgr_target = git_companion::git::Target::Local(manager.clone());
    git_companion::git::fetch::fetch_target_with_credentials(&mgr_target, "origin", &cred)
        .unwrap();
    let list = git_companion::git::mr::list_requests(&mgr_target, "origin", "main").unwrap();
    assert_eq!(list.len(), 1, "관리자 대기열에 요청이 보여야 한다");
    assert_eq!(list[0].branch, "feature/mr");
    assert_eq!(list[0].title, "MR 테스트");
    assert!(!list[0].local_only);

    // 6-5) 요청 닫기(거절·병합 완료)도 자격증명으로 원격 ref 를 지운다.
    git_companion::git::mr::close_request(&target, "origin", "main", "feature/mr", Some(&cred))
        .unwrap();
    let after = git_run(&origin, &["for-each-ref", "refs/gc-mr"]);
    assert!(
        String::from_utf8_lossy(&after.stdout).trim().is_empty(),
        "닫은 요청은 원격에서도 사라져야 한다"
    );
    // 관리자 쪽은 다음 fetch --prune 에서 대기열에서 빠진다 (앱의 자동 갱신과
    // 같은 동작).
    git_companion::git::fetch::fetch_target_with_credentials(&mgr_target, "origin", &cred)
        .unwrap();
    let after_list = git_companion::git::mr::list_requests(&mgr_target, "origin", "main").unwrap();
    assert!(after_list.is_empty(), "관리자 대기열도 비워져야 한다");

    let _ = server.kill();
}

#[test]
fn base64_encode_roundtrips() {
    use git_companion::git::ops::base64_encode;
    assert_eq!(base64_encode(b"user:pass"), "dXNlcjpwYXNz");
    // 잘 알려진 벡터
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    // 특수문자도 무손실 — python base64 로 미리 계산한 값과 같아야 한다.
    let s = "p@ss 'word/!:&%";
    assert_eq!(base64_encode(s.as_bytes()), "cEBzcyAnd29yZC8hOiYl");
}

/// 포트 0 바인딩으로 빈 포트를 고른다 (서버가 즉시 다시 바인딩).
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Basic 인증을 거는 git http-backend 서버 스크립트 (테스트 전용).
fn http_basic_git_server_py() -> &'static str {
    r#"import base64, os, subprocess, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

ROOT = sys.argv[1]
PORT = int(sys.argv[2])
USER = sys.argv[3]
PASS = sys.argv[4]
EXPECT = "Basic " + base64.b64encode((USER + ":" + PASS).encode()).decode()

class H(BaseHTTPRequestHandler):
    def _handle(self):
        if self.headers.get("Authorization", "") != EXPECT:
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Basic realm="git"')
            self.end_headers()
            return
        path = self.path.split("?")[0]
        env = dict(os.environ)
        env.update({
            "GIT_PROJECT_ROOT": ROOT,
            "GIT_HTTP_EXPORT_ALL": "1",
            "PATH_INFO": path,
            "REQUEST_METHOD": self.command,
            "QUERY_STRING": self.path.split("?", 1)[1] if "?" in self.path else "",
            "CONTENT_TYPE": self.headers.get("Content-Type", ""),
        })
        body = b""
        if self.command == "POST":
            length = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(length)
        p = subprocess.run(["git", "http-backend"], input=body, env=env, capture_output=True)
        head, _, payload = p.stdout.partition(b"\r\n\r\n")
        status = 200
        ctype = "application/octet-stream"
        for line in head.split(b"\r\n"):
            low = line.lower()
            if low.startswith(b"status:"):
                status = int(line.split()[1])
            elif low.startswith(b"content-type:"):
                ctype = line.split(b":", 1)[1].strip().decode()
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self): self._handle()
    def do_POST(self): self._handle()
    def log_message(self, *a): pass

HTTPServer(("127.0.0.1", PORT), H).serve_forever()
"#
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
fn config_store_session_and_push_credentials_roundtrip() {
    // config_store는 실제 홈 config를 쓰므로, 설정 직렬화/역직렬화로 검증한다.
    let mut s = git_companion::config_store::AppSettings::default();
    s.session = Some(git_companion::config_store::SessionState {
        user: git_companion::config_store::Account {
            id: "acc-1".into(),
            name: "홍길동".into(),
            email: "hong@example.com".into(),
            username: "hong".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            is_admin: false,
        },
        token: "tok-abc".into(),
    });
    s.push_credentials.insert(
        "repo-1".into(),
        git_companion::config_store::PushCredential {
            username: "devuser".into(),
            password: "pw".into(),
        },
    );
    let json = serde_json::to_string(&s).unwrap();
    let back: git_companion::config_store::AppSettings = serde_json::from_str(&json).unwrap();
    let session = back.session.expect("session survives roundtrip");
    assert_eq!(session.user.email, "hong@example.com");
    assert_eq!(session.token, "tok-abc");
    assert_eq!(
        back.push_credentials.get("repo-1").unwrap().username,
        "devuser"
    );
}

/// SSH 병합 대기 조회의 배치 스크립트가 로컬 구현과 **정확히 같은 결과**를
/// 내는지 — 같은 저장소에서 (1) 스크립트를 로컬 `sh` 로 돌려 파싱한 것과
/// (2) 로컬 경로 구현의 출력을 비교한다. 스크립트는 원격에서 `sh -s` 로
/// 실행되므로, 로컬 sh 로도 같은 방식으로 검증할 수 있다.
#[test]
#[cfg(unix)]
fn pending_probe_script_matches_local_implementation() {
    use git_companion::git::merge::{
        list_pending_branches, parse_pending_output, pending_probe_script,
    };

    let td = TempDir::new().unwrap();
    let root = td.path();
    init_repo(root);
    // 이 환경의 git 기본 브랜치는 master 일 수 있다 — base 를 main 으로 고정.
    git_run(root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    // base(main) + 팀원 브랜치 하나 + 이미 병합된 브랜치 하나.
    touch(&format!("{}/a.txt", root.display()));
    git_run(root, &["add", "."]);
    git_run(root, &["commit", "-q", "-m", "base commit"]);
    git_run(root, &["branch", "feature/one"]);
    git_run(root, &["branch", "merged-branch"]);

    // feature/one 에 커밋 (main 에는 없는).
    let wt = TempDir::new().unwrap(); // 별도 worktree 대신 checkout 으로 흉내
    drop(wt);
    git_run(root, &["checkout", "-q", "feature/one"]);
    touch(&format!("{}/one.txt", root.display()));
    git_run(root, &["add", "."]);
    git_run(root, &["commit", "-q", "-m", "feature work"]);
    // 브랜치 간 diff 가 name-status 에 두 줄이 나오도록 수정도 하나.
    fs::write(root.join("a.txt"), "changed").unwrap();
    git_run(root, &["add", "."]);
    git_run(root, &["commit", "-q", "-m", "edit base file"]);
    git_run(root, &["checkout", "-q", "main"]);

    // 원격 추적 브랜치 흉내: 로컬 브랜치를 refs/remotes/origin 아래에 복사.
    git_run(root, &["update-ref", "refs/remotes/origin/main", "main"]);
    git_run(root, &["update-ref", "refs/remotes/origin/feature/one", "feature/one"]);
    git_run(root, &["update-ref", "refs/remotes/origin/merged-branch", "merged-branch"]);
    git_run(root, &["update-ref", "refs/remotes/origin/HEAD", "main"]);

    let target = Target::Local(root.into());

    // (1) 로컬 구현.
    let local = list_pending_branches(&target, "origin", "main").unwrap();

    // (2) 스크립트 → sh → 파서.
    let script = pending_probe_script(&root.display().to_string(), "origin", "main");
    let out = std::process::Command::new("sh")
        .arg("-s")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.as_mut().unwrap().write_all(script.as_bytes())?;
            c.wait_with_output()
        })
        .expect("sh 실행");
    assert!(out.status.success(), "script failed: {}", String::from_utf8_lossy(&out.stderr));
    let parsed = parse_pending_output(&String::from_utf8_lossy(&out.stdout), "origin", "main")
        .expect("parse failed");

    let names = |v: &[git_companion::git::merge::PendingBranch]| {
        let mut v: Vec<String> = v.iter().map(|b| b.name.clone()).collect();
        v.sort();
        v
    };
    assert_eq!(
        names(&local),
        names(&parsed),
        "스크립트 경로와 로컬 경로의 브랜치 목록이 다릅니다\nscript stdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    for b in &parsed {
        let same = local
            .iter()
            .find(|l| l.name == b.name)
            .expect("parsed branch missing from local list");
        assert_eq!(same.sha, b.sha, "{}: sha 불일치", b.name);
        assert_eq!(same.ahead, b.ahead, "{}: ahead 불일치", b.name);
        assert_eq!(same.behind, b.behind, "{}: behind 불일치", b.name);
        assert_eq!(same.local, b.local, "{}: local 불일치", b.name);
        assert_eq!(
            same.merged_locally, b.merged_locally,
            "{}: merged_locally 불일치",
            b.name
        );
        let lf: Vec<_> = same.changed_files.iter().map(|f| (&f.kind, &f.path)).collect();
        let pf: Vec<_> = b.changed_files.iter().map(|f| (&f.kind, &f.path)).collect();
        assert_eq!(lf, pf, "{}: changed_files 불일치", b.name);
    }
    // 시나리오 검증: merged-branch 는 base에 포함되어 목록에 없어야 하고,
    // feature/one 은 대기로 잡혀야 한다.
    assert!(parsed.iter().any(|b| b.short_name == "feature/one"));
    assert!(!parsed.iter().any(|b| b.short_name == "merged-branch"));
}
