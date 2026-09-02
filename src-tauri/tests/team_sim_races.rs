//! 팀 시뮬레이션 — 동시성/레이스 시나리오.
//!
//! 6인 팀(병합 관리자 민지 + 팀원들)이 bare origin 하나를 공유하는 상황을
//! 사람별 clone 으로 재현한다. 앱이 하는 일은 전부 crate 공개 API
//! (`git_companion::git::*`)로 호출하고, raw git 은 샌드박스 구성/검증과
//! "터미널에서 사람이 직접 하는 일"(force-push, 원격 브랜치 삭제)에만 쓴다.
//!
//! BUG-n 테스트들은 수정된(올바른) 동작을 assert 하는 회귀 테스트다 —
//! 각 테스트의 주석이 원래 어떤 버그였는지 설명한다.

use std::fs;
use std::path::Path;
use std::sync::Barrier;
use tempfile::TempDir;

use git_companion::git::fetch::fetch_target;
use git_companion::git::merge::{base_unpushed_count, delete_remote_branch};
use git_companion::git::{
    complete_merge, list_pending_branches, merge_in_progress, pull, push, resolve_conflict,
    start_merge, sync_to_base, Resolution, Target,
};

// ── helpers ─────────────────────────────────────────────────────────────────

fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_try(dir, args);
    assert!(
        out.status.success(),
        "git {:?} in {} failed: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn git_try(dir: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .output()
        .unwrap()
}

fn config_user(dir: &Path, name: &str) {
    git(dir, &["config", "user.name", name]);
    git(dir, &["config", "user.email", &format!("{name}@team.example")]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

/// bare origin + 병합 관리자 민지의 작업 clone. main 에는 app.txt("line1\nline2\n")
/// 커밋 하나가 push 되어 있다.
fn team_origin() -> (TempDir, String, TempDir) {
    let bare = TempDir::new().unwrap();
    git(bare.path(), &["init", "--bare", "-q", "-b", "main"]);
    git(bare.path(), &["config", "receive.denyCurrentBranch", "ignore"]);
    let url = format!("file://{}", bare.path().display());

    let mgr = TempDir::new().unwrap();
    git(mgr.path(), &["init", "-q", "-b", "main"]);
    config_user(mgr.path(), "minji");
    seed_commit(mgr.path(), "app.txt", "line1\nline2\n", "init");
    git(mgr.path(), &["remote", "add", "origin", &url]);
    git(mgr.path(), &["push", "-q", "origin", "main"]);
    git(mgr.path(), &["fetch", "-q", "origin"]);
    (bare, url, mgr)
}

/// 팀원 한 명 = origin 의 새 clone.
fn person(url: &str, name: &str) -> TempDir {
    let td = TempDir::new().unwrap();
    git(td.path(), &["clone", "-q", url, "."]);
    config_user(td.path(), name);
    td
}

fn seed_commit(dir: &Path, file: &str, body: &str, msg: &str) {
    let p = dir.join(file);
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&p, body).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
}

fn tgt(dir: &Path) -> Target {
    Target::Local(dir.into())
}

fn head_sha(dir: &Path, r: &str) -> String {
    git(dir, &["rev-parse", r]).trim().to_string()
}

fn is_ancestor(dir: &Path, a: &str, b: &str) -> bool {
    git_try(dir, &["merge-base", "--is-ancestor", a, b])
        .status
        .success()
}

/// 팀원이 base(main)에서 브랜치를 따고 커밋 하나를 만든다. push 는 안 한다.
fn branch_with_commit(dir: &Path, branch: &str, file: &str, body: &str, msg: &str) {
    git(dir, &["checkout", "-q", "-b", branch]);
    seed_commit(dir, file, body, msg);
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 1 — 진행 중인 병합 위에 또 병합/pull/sync
// ═════════════════════════════════════════════════════════════════════════════

/// 관리자가 브랜치 A 병합 → 충돌 → 모든 충돌을 Ours(=base 유지)로 해결
/// (스테이징까지 끝, complete_merge 는 아직). 이때 두 번째 창/폴링 레이스가
/// 브랜치 B 의 start_merge 를 호출하면:
///
/// 회귀 방지(BUG-1): 과거에는 Ours 해결이 인덱스를 HEAD 와 동일하게 만들어
/// dirty-tree 가드를 통과했고, 내부의 `git checkout <base>` 가 첫 병합의
/// MERGE_HEAD 를 조용히 지워 A 의 병합(해결까지 마친 작업)이 증발했다.
/// 지금은 start_merge 가 merge_in_progress 가드로 두 번째 병합을 거부하고,
/// 첫 병합(MERGE_HEAD·스테이징된 해결 내용)이 그대로 살아남는다.
#[test]
fn bug1_start_merge_over_ours_resolved_merge_is_refused_and_first_merge_survives() {
    let (_bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());

    // 팀원 A: app.txt 만 건드리는(=충돌 파일 외 다른 변경이 없는) 브랜치.
    let a = person(&url, "memberA");
    branch_with_commit(a.path(), "feature/a", "app.txt", "line1-a\nline2\n", "feat a");
    assert!(push(&tgt(a.path()), Some("feature/a"), None).unwrap().ok);

    // 팀원 B: 독립 파일 브랜치.
    let b = person(&url, "memberB");
    branch_with_commit(b.path(), "feature/b", "b.txt", "b\n", "feat b");
    assert!(push(&tgt(b.path()), Some("feature/b"), None).unwrap().ok);

    // 관리자 main 에 충돌을 만드는 커밋.
    seed_commit(mgr.path(), "app.txt", "line1-main\nline2\n", "main edit");
    assert!(push(&mgr_t, Some("main"), None).unwrap().ok);
    fetch_target(&mgr_t, "origin").unwrap();

    // 1) A 병합 → 충돌 → 전부 Ours 로 해결(스테이징 완료, 커밋 전).
    let out_a = start_merge(&mgr_t, "origin/feature/a", "main", "origin", None).unwrap();
    assert!(out_a.conflicted);
    let remaining = resolve_conflict(&mgr_t, "app.txt", &Resolution::Ours).unwrap();
    assert!(remaining.is_empty());
    assert!(merge_in_progress(&mgr_t).unwrap(), "A 병합이 진행 중이어야 한다");

    // 2) 레이스: B 의 start_merge 가 끼어든다 — merge_in_progress 가드가 거부한다.
    let err = start_merge(&mgr_t, "origin/feature/b", "main", "origin", None)
        .expect_err("진행 중인 병합 위의 start_merge 는 거부돼야 한다");
    assert!(
        err.to_string()
            .contains("이미 진행 중인 병합이 있습니다. 병합 탭에서 먼저 마무리하거나 중단하세요."),
        "병합 진행 중 가드 메시지가 나와야 한다: {err}"
    );

    // 첫 병합(A)은 그대로 살아 있다 — MERGE_HEAD 와 스테이징된 해결 내용 보존.
    assert!(
        merge_in_progress(&mgr_t).unwrap(),
        "첫 병합의 MERGE_HEAD 가 보존되어야 한다"
    );
    let staged = git(mgr.path(), &["show", ":0:app.txt"]);
    assert_eq!(
        staged, "line1-main\nline2\n",
        "Ours 로 해결해 스테이징한 내용이 그대로 남아야 한다"
    );
    assert!(
        !is_ancestor(mgr.path(), "origin/feature/b", "main"),
        "B 는 병합되지 않았어야 한다"
    );

    // A 병합은 아무것도 잃지 않고 마무리된다.
    let done = complete_merge(&mgr_t, Some("feature/a 브랜치 병합")).unwrap();
    assert!(done.ok);
    assert!(is_ancestor(mgr.path(), "origin/feature/a", "main"));
    // A 는 로컬 병합 완료(push 대기) 상태로 표시된다 — '다시 병합' 이 아니다.
    let pending = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    let fa = pending.iter().find(|p| p.short_name == "feature/a").unwrap();
    assert!(fa.merged_locally);
}

/// 같은 상황이지만 충돌을 Manual(직접 편집한 내용)로 해결한 경우.
///
/// 회귀 방지(BUG-2): 과거에는 dirty-tree 가드가 먼저 걸려 "커밋되지 않은
/// 변경…커밋하거나 stash하세요" 라는 엉뚱한 안내가 나왔다 — 그대로 stash
/// 하면 해결해 둔 내용이 날아간다. 지금은 merge_in_progress 가드가 먼저 걸려
/// "이미 진행 중인 병합" 메시지로 올바른 경로(병합 탭에서 마무리)를 안내한다.
#[test]
fn bug2_start_merge_over_manually_resolved_merge_blocked_with_merge_guard_message() {
    let (_bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());

    let a = person(&url, "memberA");
    branch_with_commit(a.path(), "feature/a", "app.txt", "line1-a\nline2\n", "feat a");
    assert!(push(&tgt(a.path()), Some("feature/a"), None).unwrap().ok);
    let b = person(&url, "memberB");
    branch_with_commit(b.path(), "feature/b", "b.txt", "b\n", "feat b");
    assert!(push(&tgt(b.path()), Some("feature/b"), None).unwrap().ok);

    seed_commit(mgr.path(), "app.txt", "line1-main\nline2\n", "main edit");
    assert!(push(&mgr_t, Some("main"), None).unwrap().ok);
    fetch_target(&mgr_t, "origin").unwrap();

    let out_a = start_merge(&mgr_t, "origin/feature/a", "main", "origin", None).unwrap();
    assert!(out_a.conflicted);
    let manual = "line1-merged\nline2\n";
    let remaining = resolve_conflict(
        &mgr_t,
        "app.txt",
        &Resolution::Manual {
            content: manual.into(),
        },
    )
    .unwrap();
    assert!(remaining.is_empty());

    let err = start_merge(&mgr_t, "origin/feature/b", "main", "origin", None)
        .expect_err("진행 중인 병합 가드가 두 번째 병합을 막아야 한다");
    let msg = err.to_string();
    // 병합 진행 중 가드가 dirty-tree 가드보다 먼저 걸린다 — stash 를 유도하는
    // 엉뚱한 메시지 대신 병합 탭으로 안내한다.
    assert!(
        msg.contains("이미 진행 중인 병합이 있습니다. 병합 탭에서 먼저 마무리하거나 중단하세요."),
        "병합 진행 중 가드 메시지가 나와야 한다: {msg}"
    );
    assert!(
        !msg.contains("커밋되지 않은 변경"),
        "stash 를 유도하는 dirty-tree 메시지가 나오면 안 된다: {msg}"
    );

    // 첫 병합의 상태와 해결 내용은 (가드 덕에) 살아 있다.
    assert!(merge_in_progress(&mgr_t).unwrap());
    let staged = git(mgr.path(), &["show", ":0:app.txt"]);
    assert_eq!(staged, manual, "스테이징해 둔 수동 해결 내용이 보존된다");
    let done = complete_merge(&mgr_t, Some("feature/a 브랜치 병합")).unwrap();
    assert!(done.ok);
    assert!(is_ancestor(mgr.path(), "origin/feature/a", "main"));
}

/// 해결까지 끝난(커밋 전) 병합 위에서 pull()/sync_to_base() 를 부르면:
/// 둘 다 병합 상태를 파괴하지 않고, 둘 다 한국어로 안내한다.
///
/// 회귀 방지(BUG-3): 과거에는 pull 이 git 의 영어 원문("You have not concluded
/// your merge (MERGE_HEAD exists)")을 그대로 화면에 내보냈다. 지금은
/// friendly_git_error 가 진행 중인 병합 안내(한국어)로 번역한다.
#[test]
fn bug3_pull_during_resolved_merge_is_safe_and_speaks_korean() {
    let (_bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());

    let a = person(&url, "memberA");
    branch_with_commit(a.path(), "feature/a", "app.txt", "line1-a\nline2\n", "feat a");
    assert!(push(&tgt(a.path()), Some("feature/a"), None).unwrap().ok);
    seed_commit(mgr.path(), "app.txt", "line1-main\nline2\n", "main edit");
    assert!(push(&mgr_t, Some("main"), None).unwrap().ok);
    fetch_target(&mgr_t, "origin").unwrap();

    let out_a = start_merge(&mgr_t, "origin/feature/a", "main", "origin", None).unwrap();
    assert!(out_a.conflicted);
    resolve_conflict(&mgr_t, "app.txt", &Resolution::Ours).unwrap();
    assert!(merge_in_progress(&mgr_t).unwrap());

    // pull(): git 이 스스로 거부한다 — 상태는 안전(정상동작확인).
    let pulled = pull(&mgr_t).unwrap();
    assert!(!pulled.ok);
    assert!(pulled.conflicted_files.is_empty());
    assert!(
        merge_in_progress(&mgr_t).unwrap(),
        "pull 은 MERGE_HEAD 를 건드리면 안 된다"
    );
    // friendly_git_error 가 "You have not concluded your merge (MERGE_HEAD
    // exists)" 를 한국어 안내로 번역한다.
    assert!(
        pulled
            .message
            .contains("진행 중인 병합이 있습니다. 병합 탭에서 먼저 마무리하거나 중단하세요."),
        "한국어 병합 진행 안내가 나와야 한다: {}",
        pulled.message
    );
    assert!(
        !pulled.message.contains("You have not concluded your merge"),
        "영어 원문이 노출되면 안 된다: {}",
        pulled.message
    );

    // sync_to_base: 명시적 가드가 한국어로 거부한다(정상동작확인).
    let err = sync_to_base(&mgr_t, "main", "origin").expect_err("가드가 막아야 한다");
    assert!(err.to_string().contains("이미 진행 중인 병합"));
    assert!(merge_in_progress(&mgr_t).unwrap());

    // 병합은 그대로 마무리 가능 — 아무것도 잃지 않았다.
    let done = complete_merge(&mgr_t, None).unwrap();
    assert!(done.ok);
    assert!(is_ancestor(mgr.path(), "origin/feature/a", "main"));
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 2 — 검토와 병합 사이에 팀원이 커밋을 더 push (TOCTOU)
// ═════════════════════════════════════════════════════════════════════════════

/// 관리자가 목록에서 본 것(커밋 1개)과 원격의 실제 tip(커밋 3개)이 다른
/// TOCTOU 상황.
///
/// 회귀 방지(BUG-4): 과거에는 start_merge 의 내부 fetch 가 tip 을 갱신해
/// 검토하지 않은 최신 3커밋이 아무 신호 없이 병합됐다. 지금은 5번째 인자
/// expected_sha(검토 시점 tip)를 받아, fetch 후 tip 이 달라졌으면
/// "새 push가 있었습니다" 로 거부하고 저장소를 건드리지 않는다. 현재 tip 을
/// 넘기면(짧은 sha 접두사도 허용) 정상 병합된다.
#[test]
fn bug4_start_merge_with_expected_sha_refuses_when_newer_commits_arrived() {
    let (_bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());

    let member = person(&url, "memberX");
    branch_with_commit(member.path(), "feature/x", "x.txt", "x1\n", "feat x1");
    assert!(push(&tgt(member.path()), Some("feature/x"), None).unwrap().ok);

    // 관리자: 가져오기 → 검토. 커밋 1개, 파일은 x.txt 뿐.
    fetch_target(&mgr_t, "origin").unwrap();
    let pending = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    let reviewed = pending.iter().find(|p| p.short_name == "feature/x").unwrap();
    assert_eq!(reviewed.ahead, 1);
    let reviewed_sha = reviewed.sha.clone();

    // 그 사이 팀원이 커밋 2개를 더 push.
    seed_commit(member.path(), "x2.txt", "x2\n", "feat x2");
    seed_commit(member.path(), "x3.txt", "x3\n", "feat x3");
    assert!(push(&tgt(member.path()), Some("feature/x"), None).unwrap().ok);
    let newest_sha = head_sha(member.path(), "HEAD");

    // 관리자 화면(폴링 전)은 여전히 옛 모습 — 바뀌었다는 신호가 없다.
    let stale = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    let still = stale.iter().find(|p| p.short_name == "feature/x").unwrap();
    assert_eq!(still.ahead, 1, "목록은 마지막 fetch 시점 그대로다");
    assert_eq!(still.sha, reviewed_sha);

    // 병합 버튼(검토 시점 sha 전달): 내부 fetch 가 tip 변화를 감지해 거부한다.
    let head_before = head_sha(mgr.path(), "main");
    let err = start_merge(&mgr_t, "origin/feature/x", "main", "origin", Some(&reviewed_sha))
        .expect_err("검토 후 tip 이 움직였으면 병합을 거부해야 한다");
    assert!(
        err.to_string().contains("새 push가 있었습니다"),
        "TOCTOU 안내 메시지: {err}"
    );

    // 저장소는 건드리지 않았다 — 병합 상태 없음, main 그대로, 트리 깨끗.
    assert!(!merge_in_progress(&mgr_t).unwrap());
    assert_eq!(head_sha(mgr.path(), "main"), head_before);
    assert!(!is_ancestor(mgr.path(), &newest_sha, "main"));
    let status = git(mgr.path(), &["status", "--porcelain"]);
    assert!(status.trim().is_empty(), "트리가 더러워지면 안 된다: {status}");

    // 새로고침 후 현재 tip 을 넘기면(짧은 접두사 허용) 병합은 정상 진행된다.
    let out = start_merge(&mgr_t, "origin/feature/x", "main", "origin", Some(&newest_sha[..7]))
        .unwrap();
    assert!(out.ok, "현재 tip 을 확인하고 병합하면 성공해야 한다: {}", out.message);
    assert!(is_ancestor(mgr.path(), &newest_sha, "main"));
    assert!(mgr.path().join("x2.txt").exists());
    assert!(mgr.path().join("x3.txt").exists());
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 3 — 두 사람이 동시에 병합하고 둘 다 push
// ═════════════════════════════════════════════════════════════════════════════

/// 관리자(clone1)가 A 를, 독단적인 팀원(clone2)이 B 를 같은 시점의 main 위에
/// 병합한다. 먼저 push 한 쪽이 이기고, 두 번째 push 는 non-fast-forward 로
/// 거부되며, 앱 메시지가 '동기화' 를 안내한다. sync_to_base → push 로 복구되고
/// 커밋은 하나도 사라지지 않는다. (정상동작확인)
#[test]
fn simultaneous_merges_second_push_rejected_then_sync_recovers() {
    let (bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());

    let a = person(&url, "memberA");
    branch_with_commit(a.path(), "feature/a", "a.txt", "a\n", "feat a");
    assert!(push(&tgt(a.path()), Some("feature/a"), None).unwrap().ok);
    let a_sha = head_sha(a.path(), "HEAD");

    let rogue = person(&url, "rogue");
    branch_with_commit(rogue.path(), "feature/b", "b.txt", "b\n", "feat b");
    let rogue_t = tgt(rogue.path());
    assert!(push(&rogue_t, Some("feature/b"), None).unwrap().ok);
    let b_sha = head_sha(rogue.path(), "HEAD");

    // 둘 다 "같은 origin/main" 위에 병합한다 (아직 아무도 push 전).
    fetch_target(&mgr_t, "origin").unwrap();
    assert!(start_merge(&mgr_t, "origin/feature/a", "main", "origin", None).unwrap().ok);
    assert!(start_merge(&rogue_t, "origin/feature/b", "main", "origin", None).unwrap().ok);

    // 관리자가 먼저 push — 성공.
    assert!(push(&mgr_t, Some("main"), None).unwrap().ok);

    // 팀원의 push — non-fast-forward 로 거부, 메시지가 동기화를 안내해야 한다.
    let lost = push(&rogue_t, Some("main"), None).unwrap();
    assert!(!lost.ok, "두 번째 push 는 실패해야 한다");
    assert!(!lost.auth_required);
    assert!(
        lost.message.contains("동기화"),
        "복구 경로(동기화)를 안내해야 한다: {}",
        lost.message
    );

    // 복구: sync_to_base → push.
    let sync = sync_to_base(&rogue_t, "main", "origin").unwrap();
    assert!(!sync.conflicted, "독립 파일이므로 충돌 없음: {}", sync.message);
    assert!(push(&rogue_t, Some("main"), None).unwrap().ok);

    // origin/main 에 두 병합이 모두 살아 있다 — 커밋 손실 없음.
    assert!(is_ancestor(bare.path(), &a_sha, "main"));
    assert!(is_ancestor(bare.path(), &b_sha, "main"));
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 4 — 진짜 동시 push (std::thread + Barrier)
// ═════════════════════════════════════════════════════════════════════════════

/// 팀원 3명이 서로 다른 브랜치를 문자 그대로 동시에 push — 전부 성공해야 하고
/// origin 은 손상되지 않아야 한다. (정상동작확인)
#[test]
fn three_members_push_distinct_branches_concurrently() {
    let (bare, url, _mgr) = team_origin();

    let members: Vec<(TempDir, String)> = (1..=3)
        .map(|i| {
            let p = person(&url, &format!("member{i}"));
            let branch = format!("feature/m{i}");
            branch_with_commit(p.path(), &branch, &format!("m{i}.txt"), "w\n", "work");
            (p, branch)
        })
        .collect();
    let expected: Vec<String> = members
        .iter()
        .map(|(p, _)| head_sha(p.path(), "HEAD"))
        .collect();

    let barrier = Barrier::new(3);
    let outcomes: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = members
            .iter()
            .map(|(p, branch)| {
                let barrier = &barrier;
                let t = tgt(p.path());
                s.spawn(move || {
                    barrier.wait();
                    push(&t, Some(branch), None).unwrap()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    for (i, o) in outcomes.iter().enumerate() {
        assert!(o.ok, "member{} push 실패: {}", i + 1, o.message);
    }
    // origin 검증: ref 3개가 정확한 sha 를 가리키고, 저장소가 깨지지 않았다.
    for (i, sha) in expected.iter().enumerate() {
        let got = head_sha(bare.path(), &format!("refs/heads/feature/m{}", i + 1));
        assert_eq!(&got, sha);
    }
    let fsck = git_try(bare.path(), &["fsck", "--strict"]);
    assert!(fsck.status.success(), "origin 손상: {}", String::from_utf8_lossy(&fsck.stderr));
}

/// 두 팀원이 **같은 브랜치**에 서로 다른 커밋을 동시에 push — 정확히 한 명만
/// 이기고, 진 쪽은 친절한 한국어 PushOutcome 메시지(동기화 안내)를 받아야
/// 한다. origin 은 승자의 sha 를 가리키고 손상이 없어야 한다. (정상동작확인)
#[test]
fn two_members_push_same_branch_concurrently_one_wins_loser_gets_korean_message() {
    let (bare, url, _mgr) = team_origin();

    let p1 = person(&url, "member1");
    branch_with_commit(p1.path(), "feature/shared", "s.txt", "one\n", "one");
    let p2 = person(&url, "member2");
    branch_with_commit(p2.path(), "feature/shared", "s.txt", "two\n", "two");
    let shas = [head_sha(p1.path(), "HEAD"), head_sha(p2.path(), "HEAD")];

    let barrier = Barrier::new(2);
    let outcomes: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = [&p1, &p2]
            .into_iter()
            .map(|p| {
                let barrier = &barrier;
                let t = tgt(p.path());
                s.spawn(move || {
                    barrier.wait();
                    push(&t, Some("feature/shared"), None).unwrap()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let winners: Vec<usize> = (0..2).filter(|&i| outcomes[i].ok).collect();
    assert_eq!(winners.len(), 1, "정확히 한 명만 성공해야 한다: {outcomes:?}");
    let winner = winners[0];
    let loser = 1 - winner;

    // 진 쪽 메시지: friendly_git_error 를 거친 한국어 안내여야 한다.
    let msg = &outcomes[loser].message;
    assert!(!outcomes[loser].auth_required);
    assert!(
        msg.contains("동기화") && (msg.contains("푸시 거부됨") || msg.contains("푸시 실패")),
        "진 쪽은 한국어 동기화 안내를 받아야 한다: {msg}"
    );
    assert!(
        !msg.contains("fast-forward") && !msg.contains("rejected"),
        "영어 원문이 노출되면 안 된다: {msg}"
    );

    // origin: 승자의 sha, 손상 없음.
    assert_eq!(head_sha(bare.path(), "refs/heads/feature/shared"), shas[winner]);
    let fsck = git_try(bare.path(), &["fsck", "--strict"]);
    assert!(fsck.status.success());
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 5 — 관리자 발밑에서 force-push (히스토리 재작성)
// ═════════════════════════════════════════════════════════════════════════════

/// 관리자가 fetch 해 둔 뒤 팀원이 브랜치 히스토리를 갈아엎고 force-push.
/// list_pending_branches 는 (마지막 fetch 기준) 옛 sha/ahead 를 보여 준다.
///
/// 회귀 방지(BUG-5): 과거에는 start_merge 의 내부 fetch 가 재작성된 새
/// 히스토리를 아무 신호 없이 병합했다. 지금은 expected_sha(검토 시점 tip)를
/// 넘기면 "새 push가 있었습니다" 로 거부하고 저장소를 건드리지 않는다.
/// expected_sha=None 이면 예전처럼 최신 히스토리를 병합한다(기존 호출부 보존).
#[test]
fn bug5_force_pushed_branch_refused_with_expected_sha_merged_with_none() {
    let (_bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());

    let member = person(&url, "memberF");
    branch_with_commit(member.path(), "feature/f", "f1.txt", "f1\n", "feat f1");
    seed_commit(member.path(), "f2.txt", "f2\n", "feat f2");
    assert!(push(&tgt(member.path()), Some("feature/f"), None).unwrap().ok);
    let old_sha = head_sha(member.path(), "HEAD");

    // 관리자: 가져오기 → 검토 (커밋 2개짜리로 보인다).
    fetch_target(&mgr_t, "origin").unwrap();
    let pending = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    let reviewed = pending.iter().find(|p| p.short_name == "feature/f").unwrap();
    assert_eq!(reviewed.ahead, 2);
    assert_eq!(reviewed.sha, old_sha);

    // 팀원이 터미널에서 히스토리를 갈아엎고 force-push (앱 밖의 행동 → raw git).
    git(member.path(), &["reset", "-q", "--hard", "origin/main"]);
    seed_commit(member.path(), "f-new.txt", "rewritten\n", "feat f rewritten");
    git(member.path(), &["push", "-q", "--force", "origin", "feature/f"]);
    let new_sha = head_sha(member.path(), "HEAD");

    // 관리자 목록은 아무 일 없었다는 듯 옛 모습 그대로 — 패닉/오류 없음.
    let stale = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    let still = stale.iter().find(|p| p.short_name == "feature/f").unwrap();
    assert_eq!(still.sha, old_sha, "마지막 fetch 기준의 옛 sha");
    assert_eq!(still.ahead, 2);

    // 검토 시점 sha(old_sha)를 넘기면: 내부 fetch 가 (강제 갱신 refspec 으로)
    // 트래킹 ref 를 새 히스토리로 바꾼 뒤, tip 이 검토와 다름을 감지해 거부한다.
    let head_before = head_sha(mgr.path(), "main");
    let err = start_merge(&mgr_t, "origin/feature/f", "main", "origin", Some(&old_sha))
        .expect_err("히스토리가 재작성된 브랜치는 검토 sha 와 달라 거부돼야 한다");
    assert!(
        err.to_string().contains("새 push가 있었습니다"),
        "재작성 감지 안내: {err}"
    );
    assert!(!merge_in_progress(&mgr_t).unwrap());
    assert_eq!(head_sha(mgr.path(), "main"), head_before, "저장소는 그대로여야 한다");
    assert!(!is_ancestor(mgr.path(), &new_sha, "main"));

    // expected_sha=None: 기존 동작 그대로 — 최신(재작성된) 히스토리를 병합한다.
    let out = start_merge(&mgr_t, "origin/feature/f", "main", "origin", None).unwrap();
    assert!(out.ok, "None 이면 최신 tip 을 그대로 병합한다: {}", out.message);
    assert!(is_ancestor(mgr.path(), &new_sha, "main"));
    assert!(
        !is_ancestor(mgr.path(), &old_sha, "main"),
        "검토했던 f1/f2 커밋은 main 에 들어가지 않았다"
    );
    assert!(mgr.path().join("f-new.txt").exists());
    assert!(!mgr.path().join("f1.txt").exists());
    // 병합 후 목록/상태는 정상 — 손상 없음 확인. (main 을 아직 push 하지
    // 않았으므로 feature/f 는 merged_locally=true 인 '푸시 대기' 로 남는다.)
    assert!(!merge_in_progress(&mgr_t).unwrap());
    let after = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    let ff = after.iter().find(|p| p.short_name == "feature/f").unwrap();
    assert!(ff.merged_locally);
    assert!(push(&mgr_t, Some("main"), None).unwrap().ok);
    fetch_target(&mgr_t, "origin").unwrap();
    let after = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    assert!(after.iter().all(|p| p.short_name != "feature/f"));
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 6 — 보고 있던 원격 브랜치가 발밑에서 삭제됨
// ═════════════════════════════════════════════════════════════════════════════

/// 관리자 1이 대기 목록의 브랜치 X 를 보는 사이 다른 사람이 X 를 지운다.
/// delete_remote_branch 는 미병합 브랜치를 거부한다(정상). 터미널로 지워진 뒤
/// start_merge 를 누르면:
///
/// 회귀 방지(BUG-6): 과거에는 "merge failed: … not something we can merge"
/// 영어 원문이 그대로 노출됐다. 지금은 fetch --prune 뒤 tip 검증이 사라진
/// ref 를 감지해 "방금 원격에서 삭제되었을 수 있습니다 — 새로고침" 한국어
/// 안내를 준다. 저장소는 안전하게 남는다.
#[test]
fn bug6_start_merge_on_branch_deleted_underfoot_fails_with_korean_guidance() {
    let (_bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());

    let member = person(&url, "memberG");
    branch_with_commit(member.path(), "feature/gone", "g.txt", "g\n", "feat gone");
    assert!(push(&tgt(member.path()), Some("feature/gone"), None).unwrap().ok);

    // 관리자 1: 가져오기 → 목록에 보인다.
    fetch_target(&mgr_t, "origin").unwrap();
    let pending = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    assert!(pending.iter().any(|p| p.short_name == "feature/gone"));
    let head_before = head_sha(mgr.path(), "main");

    // 관리자 2: 앱의 삭제 기능은 미병합 브랜치를 거부한다 (정상동작확인 —
    // 팀원 커밋을 지키는 안전장치가 레이스에서도 동작).
    let mgr2 = person(&url, "manager2");
    let err = delete_remote_branch(&tgt(mgr2.path()), "origin", "main", "feature/gone")
        .expect_err("미병합 브랜치 삭제는 거부돼야 한다");
    assert!(err.to_string().contains("없는 커밋"));

    // 그래서 관리자 2가 터미널에서 지워 버린다 (앱 밖 행동 → raw git).
    git(mgr2.path(), &["push", "-q", "origin", "--delete", "feature/gone"]);

    // 관리자 1이 (여전히 목록에 보이는) X 의 병합 버튼을 누른다.
    let err = start_merge(&mgr_t, "origin/feature/gone", "main", "origin", None)
        .expect_err("사라진 ref 병합은 실패해야 한다");
    let msg = err.to_string();
    // 사라진 ref 는 한국어 안내(삭제 추정 + 새로고침 권유)로 설명된다.
    assert!(
        msg.contains("origin/feature/gone 브랜치를 찾을 수 없습니다")
            && msg.contains("방금 원격에서 삭제되었을 수 있습니다")
            && msg.contains("새로고침"),
        "사라진 브랜치 한국어 안내가 나와야 한다: {msg}"
    );
    assert!(
        !msg.contains("not something we can merge"),
        "영어 원문이 노출되면 안 된다: {msg}"
    );

    // 저장소는 멀쩡하다: 병합 상태 없음, 트리 깨끗, HEAD 그대로.
    assert!(!merge_in_progress(&mgr_t).unwrap());
    assert_eq!(head_sha(mgr.path(), "main"), head_before);
    let status = git(mgr.path(), &["status", "--porcelain"]);
    assert!(status.trim().is_empty(), "트리가 더러워지면 안 된다: {status}");
    // 내부 fetch --prune 덕에 목록에서도 사라져 있다.
    let after = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    assert!(after.iter().all(|p| p.short_name != "feature/gone"));
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 7 — base(origin/main)가 되감기거나 재작성됨
// ═════════════════════════════════════════════════════════════════════════════

/// origin/main 이 강제로 되감기고(2커밋 뒤로), 이후 다른 히스토리로 재작성돼도
/// 팀원의 sync_to_base 는 로컬 커밋을 잃지 않고, base_unpushed_count 는
/// (마지막 fetch 기준으로) 진실한 값을 준다. (정상동작확인)
#[test]
fn base_force_rewound_sync_loses_nothing_and_counts_stay_truthful() {
    let (bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());
    seed_commit(mgr.path(), "c2.txt", "c2\n", "c2");
    seed_commit(mgr.path(), "c3.txt", "c3\n", "c3");
    assert!(push(&mgr_t, Some("main"), None).unwrap().ok);
    let c3 = head_sha(mgr.path(), "main");

    let member = person(&url, "memberS"); // main = c3 를 갖고 시작
    let member_t = tgt(member.path());

    // 악당이 터미널에서 main 을 2커밋 되감아 force-push.
    let rogue = person(&url, "rogue");
    git(rogue.path(), &["reset", "-q", "--hard", "HEAD~2"]);
    git(rogue.path(), &["push", "-q", "--force", "origin", "main"]);
    let c1 = head_sha(rogue.path(), "main");

    // fetch 전: 카운트는 마지막 fetch 기준 → 0.
    assert_eq!(base_unpushed_count(&member_t, "origin", "main").unwrap(), 0);
    // fetch 후: 원격이 잃어버린 2커밋이 "unpushed" 로 잡힌다 — 진실한 값.
    fetch_target(&member_t, "origin").unwrap();
    assert_eq!(base_unpushed_count(&member_t, "origin", "main").unwrap(), 2);

    // sync_to_base: origin/main(c1) 은 이미 조상 → 아무것도 잃지 않는다.
    let sync = sync_to_base(&member_t, "main", "origin").unwrap();
    assert!(!sync.conflicted, "{}", sync.message);
    assert_eq!(head_sha(member.path(), "HEAD"), c3, "로컬 c2/c3 보존");
    // push 로 되감긴 원격을 복구할 수 있다.
    assert!(push(&member_t, Some("main"), None).unwrap().ok);
    assert_eq!(head_sha(bare.path(), "refs/heads/main"), c3);

    // 2막: 이번엔 c1 위에 **다른** 커밋을 얹어 재작성 force-push (발산 히스토리).
    git(rogue.path(), &["reset", "-q", "--hard", &c1]);
    seed_commit(rogue.path(), "rogue.txt", "r\n", "rewritten base");
    git(rogue.path(), &["push", "-q", "--force", "origin", "main"]);
    let c4 = head_sha(rogue.path(), "main");

    fetch_target(&member_t, "origin").unwrap();
    let sync = sync_to_base(&member_t, "main", "origin").unwrap();
    assert!(!sync.conflicted, "파일이 겹치지 않으므로 충돌 없음: {}", sync.message);
    // 병합 커밋이 양쪽 히스토리를 모두 보존한다 — 데이터 손실 없음.
    let head = head_sha(member.path(), "HEAD");
    assert!(is_ancestor(member.path(), &c3, &head));
    assert!(is_ancestor(member.path(), &c4, &head));
    // 카운트: 원격에 없는 c2+c3+병합커밋 = 3. 놀랍지만 거짓은 아니다.
    assert_eq!(base_unpushed_count(&member_t, "origin", "main").unwrap(), 3);
    assert!(push(&member_t, Some("main"), None).unwrap().ok);
    assert!(is_ancestor(bare.path(), &c3, "main"));
    assert!(is_ancestor(bare.path(), &c4, "main"));
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 8 — 병합된 base 의 push 가 도중에 실패, 그 사이 팀원 브랜치가 계속 도착
// ═════════════════════════════════════════════════════════════════════════════

/// 관리자가 feature/m 을 로컬 main 에 병합했는데 push 가 (네트워크 등으로)
/// 실패했다. 그 사이 팀원이 feature/y 를 push 해도 merged_locally 플래그와
/// base_unpushed_count 는 계속 진실해야 한다. (정상동작확인)
#[test]
fn merged_locally_flag_and_unpushed_count_stay_truthful_while_branches_keep_landing() {
    let (_bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());

    let m = person(&url, "memberM");
    branch_with_commit(m.path(), "feature/m", "m.txt", "m\n", "feat m");
    assert!(push(&tgt(m.path()), Some("feature/m"), None).unwrap().ok);

    fetch_target(&mgr_t, "origin").unwrap();
    assert!(start_merge(&mgr_t, "origin/feature/m", "main", "origin", None).unwrap().ok);

    // push 가 도중에 실패한다 (원격이 잠시 손상/유실된 상황을 URL 로 재현).
    let real_url = git(mgr.path(), &["remote", "get-url", "origin"]).trim().to_string();
    git(mgr.path(), &["remote", "set-url", "origin", "file:///nonexistent/gc-race-origin"]);
    let failed = push(&mgr_t, Some("main"), None).unwrap();
    assert!(!failed.ok, "push 는 실패해야 한다");
    git(mgr.path(), &["remote", "set-url", "origin", &real_url]);

    // 그 사이 팀원이 새 브랜치를 push.
    let y = person(&url, "memberY");
    branch_with_commit(y.path(), "feature/y", "y.txt", "y\n", "feat y");
    assert!(push(&tgt(y.path()), Some("feature/y"), None).unwrap().ok);

    // 관리자 화면 재구성: 가져오기 → 목록/카운트.
    fetch_target(&mgr_t, "origin").unwrap();
    let pending = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    let fm = pending.iter().find(|p| p.short_name == "feature/m").unwrap();
    assert!(
        fm.merged_locally,
        "feature/m 은 '다시 병합' 이 아니라 '푸시 대기' 로 보여야 한다"
    );
    let fy = pending.iter().find(|p| p.short_name == "feature/y").unwrap();
    assert!(!fy.merged_locally);
    assert_eq!(fy.ahead, 1);
    assert_eq!(
        base_unpushed_count(&mgr_t, "origin", "main").unwrap(),
        2,
        "병합 커밋 + feat m 커밋"
    );

    // push 재시도 성공 → 플래그/카운트가 정리되고 feature/y 만 남는다.
    assert!(push(&mgr_t, Some("main"), None).unwrap().ok);
    fetch_target(&mgr_t, "origin").unwrap();
    assert_eq!(base_unpushed_count(&mgr_t, "origin", "main").unwrap(), 0);
    let pending = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    assert!(pending.iter().all(|p| p.short_name != "feature/m"));
    assert!(pending.iter().any(|p| p.short_name == "feature/y"));
}
