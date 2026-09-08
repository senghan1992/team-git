//! 병합 요청(merge request) 라이프사이클 — 푸시와 승인을 분리하는 대기열.
//!
//! 흐름: 팀원이 push → **병합 요청 보내기**(refs/gc-mr/<base>/<branch>) →
//! 관리자 대기열에 요청이 뜬다 → 승인(병합)하면 요청은 닫히고(원각에서도
//! 사라진다) 거절하면 그냥 닫힌다. 요청은 푸시된 커밋에만 할 수 있고,
//! 이미 병합된 브랜치·새 커밋이 없는 브랜치는 거부된다.
use std::fs;
use std::path::Path;
use tempfile::TempDir;

use git_companion::git::merge::start_merge;
use git_companion::git::mr::{
    close_request, list_requested_merges, list_requests, request_merge,
};
use git_companion::git::ops::push;
use git_companion::git::Target;

fn git_run(dir: &Path, args: &[&str]) -> std::process::Output {
    let mut c = std::process::Command::new("git");
    c.args(args)
        .current_dir(dir)
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8");
    c.output().unwrap()
}

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
fn request_roundtrip_list_and_merge_closes_it() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/x"]);
    seed_commit(work.path(), "x.txt", "x\n", "feat x");

    // 요청 전: 푸시해야 한다 — 요청은 푸시된 커밋만 받는다.
    let target = Target::Local(work.path().into());
    let err = request_merge(&target, "origin", "main", "feature/x", None, "민지", "m@x")
        .expect_err("푸시 없이 요청하면 거부된다");
    assert!(format!("{err}").contains("푸시"));

    git_run(work.path(), &["push", "-q", "origin", "feature/x"]);
    let req = request_merge(&target, "origin", "main", "feature/x", None, "민지", "m@x")
        .expect("push 후 요청 성공");
    assert_eq!(req.branch, "feature/x");
    assert_eq!(req.base, "main");
    assert!(req.ref_path.starts_with("refs/gc-mr/main/feature/x"));
    // 제목 기본값 = 마지막 커밋 제목.
    assert_eq!(req.title, "feat x");

    // 같은 브랜치에 다시 요청하면 갱신이다 (중복 ref가 생기지 않는다).
    let req2 = request_merge(&target, "origin", "main", "feature/x", Some("로그인 수정"), "민지", "m@x")
        .unwrap();
    assert_eq!(req2.sha, req.sha, "새 push 없이 재요청하면 tip은 그대로");
    assert_eq!(req2.title, "로그인 수정", "제목은 갱신된다");

    // push 후 다시 요청하면 tip이 갱신된다 — 관리자는 항상 요청된 tip을 본다.
    seed_commit(work.path(), "y.txt", "y\n", "feat x more");
    git_run(work.path(), &["push", "-q", "origin", "feature/x"]);
    let req3 = request_merge(&target, "origin", "main", "feature/x", None, "민지", "m@x").unwrap();
    assert_ne!(req3.sha, req2.sha, "새 push 반영된 tip으로 갱신");

    let queue = list_requested_merges(&target, "origin", "main").unwrap();
    assert_eq!(queue.len(), 1, "브랜치 하나당 요청 하나");
    let rm = &queue[0];
    assert_eq!(rm.request.branch, "feature/x");
    assert_eq!(rm.ahead, 2, "요청 tip 기준 ahead");
    let paths: Vec<&str> = rm.changed_files.iter().map(|c| c.path.as_str()).collect();
    assert!(paths.contains(&"x.txt") && paths.contains(&"y.txt"));

    // 관리자 승인 = 요청 ref를 병합. expected_sha로 검토 시점을 고정한다.
    let out = start_merge(&target, &req3.ref_path, "main", "origin", Some(&req3.sha)).unwrap();
    assert!(out.ok, "요청 ref 병합 성공: {}", out.message);
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["fetch", "-q", "origin"]);

    // 승인 후 요청을 닫는다 — 대기열과 원격에서 모두 사라진다.
    close_request(&target, "origin", "main", "feature/x").unwrap();
    assert!(list_requests(&target, "origin", "main").unwrap().is_empty());
    let check = git_run(bare.path(), &["rev-parse", "-q", "--verify", "refs/gc-mr/main/feature/x"]);
    assert!(!check.status.success(), "원격에서도 요청 ref가 지워진다");
}

#[test]
fn request_rejects_merged_and_no_change_branches() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);

    // base와 같은 커밋을 가리키는 브랜치 — 병합할 것이 없다 (조상이므로
    // "이미 병합"으로 거부된다).
    git_run(work.path(), &["checkout", "-q", "-b", "nochange"]);
    git_run(work.path(), &["push", "-q", "origin", "nochange"]);
    let target = Target::Local(work.path().into());
    let err = request_merge(&target, "origin", "main", "nochange", None, "민지", "m@x")
        .expect_err("새 커밋이 없으면 거부");
    assert!(format!("{err}").contains("이미"));

    // 이미 base에 병합된 브랜치.
    git_run(work.path(), &["checkout", "-q", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "merged"]);
    seed_commit(work.path(), "m.txt", "m\n", "merged work");
    git_run(work.path(), &["push", "-q", "origin", "merged"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    git_run(work.path(), &["merge", "-q", "--no-ff", "merged"]);
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["fetch", "-q", "origin"]);
    let err = request_merge(&target, "origin", "main", "merged", None, "민지", "m@x")
        .expect_err("이미 병합된 브랜치는 거부");
    assert!(format!("{err}").contains("이미"));

    // 푸시하지 않은 커밋이 있으면 거부 — 승인 대상이 모호해지기 때문.
    git_run(work.path(), &["checkout", "-q", "-b", "dirty"]);
    seed_commit(work.path(), "d.txt", "d\n", "unpushed work");
    let err = request_merge(&target, "origin", "main", "dirty", None, "민지", "m@x")
        .expect_err("origin 브랜치가 없으면 거부");
    assert!(format!("{err}").contains("푸시"));
}

#[test]
fn list_auto_closes_requests_merged_outside_the_app() {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/t"]);
    seed_commit(work.path(), "t.txt", "t\n", "t work");
    git_run(work.path(), &["push", "-q", "origin", "feature/t"]);
    let target = Target::Local(work.path().into());
    request_merge(&target, "origin", "main", "feature/t", None, "민지", "m@x").unwrap();
    assert_eq!(list_requests(&target, "origin", "main").unwrap().len(), 1);

    // 앱 밖에서 병합됐다 (터미널).
    git_run(work.path(), &["checkout", "-q", "main"]);
    git_run(work.path(), &["merge", "-q", "--no-ff", "origin/feature/t"]);
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["fetch", "-q", "origin"]);

    let queue = list_requests(&target, "origin", "main").unwrap();
    assert!(
        queue.is_empty(),
        "이미 병합된 요청은 대기열에서 자동으로 치워진다"
    );
    let check = git_run(bare.path(), &["rev-parse", "-q", "--verify", "refs/gc-mr/main/feature/t"]);
    assert!(!check.status.success(), "원격 요청 ref도 정리된다");
}

#[test]
fn push_notification_fires_on_branch_push_but_ref_is_separate() {
    // 병합 대상에 push해도(gc-mr ref가 아니라) 요청은 생기지 않는다 —
    // 푸시와 요청이 분리돼 있다는 것의 회귀 테스트.
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/silent"]);
    seed_commit(work.path(), "s.txt", "s\n", "silent work");
    git_run(work.path(), &["push", "-q", "origin", "feature/silent"]);

    let target = Target::Local(work.path().into());
    assert!(
        list_requests(&target, "origin", "main").unwrap().is_empty(),
        "푸시만으로는 요청이 생기지 않는다"
    );
    // 하지만 병합 센터의 하위 호환 조회(푸시된 브랜치 목록)에는 보인다 —
    // 다른 화면(타임라인 등)이 여전히 쓰는 데이터다.
    let pending =
        git_companion::git::merge::list_pending_branches(&target, "origin", "main").unwrap();
    assert!(pending.iter().any(|b| b.short_name == "feature/silent"));

    // 요청해야 대기열에 오른다.
    request_merge(&target, "origin", "main", "feature/silent", None, "민지", "m@x").unwrap();
    let queue = list_requested_merges(&target, "origin", "main").unwrap();
    assert_eq!(queue.len(), 1);
    // push()로 base를 밀 때처럼 푸시가 겹쳐도 요청 ref는 별도다.
    push(&target, Some("feature/silent"), None).unwrap();
    assert_eq!(list_requests(&target, "origin", "main").unwrap().len(), 1);
}
