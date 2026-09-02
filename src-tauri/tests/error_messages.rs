//! git 오류 → 사람 말 변환.
//!
//! 처음 git 을 쓰는 사람이 실제로 마주치는 실패들이다. 한국어 화면에 영어
//! `fatal:` 줄이 그대로 뜨거나 메시지가 빈 문자열이면, 무엇이 잘못됐는지도
//! 무엇을 해야 하는지도 알 수 없다.

use git_companion::git::ops::{explain_commit_failure, friendly_git_error};

/// 사용자에게 보이는 문구가 갖춰야 할 최소 조건.
fn assert_human(msg: &str) {
    assert!(!msg.trim().is_empty(), "빈 메시지를 보여 주면 안 된다");
    assert!(
        !msg.contains("fatal:"),
        "git 의 영어 원문이 그대로 새어 나왔다: {msg}"
    );
    assert!(
        msg.chars().any(|c| ('가'..='힣').contains(&c)),
        "한국어 설명이 없다: {msg}"
    );
}

// ── 커밋 ─────────────────────────────────────────────────────────────────────

/// git 은 "nothing to commit" 을 **stdout** 에 쓴다. stderr 만 읽으면 빈
/// 문자열이 되어 화면에 "커밋 실패: " 만 뜬다 — 커밋할 변경이 없을 때 버튼을
/// 누르는 것은 처음 쓰는 사람이 가장 자주 하는 일이다.
#[test]
fn nothing_to_commit_is_explained_from_stdout() {
    let stdout = "On branch main\nnothing to commit, working tree clean\n";
    let msg = explain_commit_failure(stdout, "");
    assert_human(&msg);
    assert!(msg.contains("변경이 없습니다"), "got: {msg}");
}

#[test]
fn staged_nothing_variant_is_explained() {
    let stdout = "On branch main\nno changes added to commit (use \"git add\")\n";
    let msg = explain_commit_failure(stdout, "");
    assert_human(&msg);
    assert!(msg.contains("변경이 없습니다"), "got: {msg}");
}

#[test]
fn empty_commit_message_is_explained() {
    let msg = explain_commit_failure("", "Aborting commit due to empty commit message.\n");
    assert_human(&msg);
    assert!(msg.contains("메시지"), "got: {msg}");
}

#[test]
fn missing_git_identity_tells_the_user_what_to_run() {
    let stderr = "Author identity unknown\n*** Please tell me who you are.\n";
    let msg = explain_commit_failure("", stderr);
    assert_human(&msg);
    assert!(
        msg.contains("user.email"),
        "설정 방법을 알려 줘야 한다: {msg}"
    );
}

#[test]
fn unmerged_paths_point_at_the_merge_tab() {
    let msg = explain_commit_failure(
        "",
        "error: Committing is not possible because you have unmerged files.\n",
    );
    assert_human(&msg);
    assert!(msg.contains("병합"), "got: {msg}");
}

#[test]
fn unknown_commit_failure_never_returns_empty() {
    // 원인을 몰라도 빈 문자열을 돌려주면 안 된다.
    let msg = explain_commit_failure("", "");
    assert!(!msg.trim().is_empty());
    let raw = explain_commit_failure("", "error: something very unusual happened");
    assert!(
        raw.contains("something very unusual"),
        "git 이 한 말은 남겨 준다"
    );
}

// ── 푸시 / 원격 ──────────────────────────────────────────────────────────────

/// 혼자 `git init` 한 저장소에서 푸시를 누르면 나오는 실제 출력.
/// 예전에는 이 영어 5줄이 그대로 화면에 떴다.
#[test]
fn missing_remote_is_explained_and_shows_the_fix() {
    let stderr = "fatal: 'origin' does not appear to be a git repository\n\
                  fatal: Could not read from remote repository.\n\n\
                  Please make sure you have the correct access rights\n\
                  and the repository exists.\n";
    let msg = friendly_git_error(stderr);
    assert_human(&msg);
    assert!(msg.contains("원격"), "got: {msg}");
    assert!(
        msg.contains("git remote add"),
        "무엇을 해야 하는지 알려 줘야 한다: {msg}"
    );
    // "access rights" 때문에 인증 실패로 오분류되면 안 된다.
    assert!(
        !msg.contains("SSH 키"),
        "원격 부재를 인증 문제로 말하면 안 된다: {msg}"
    );
}

#[test]
fn no_commits_to_push_is_explained() {
    let msg = friendly_git_error("error: src refspec main does not match any\n");
    assert_human(&msg);
    assert!(msg.contains("커밋"), "got: {msg}");
}

#[test]
fn rejected_push_points_at_sync() {
    let msg = friendly_git_error(
        "! [rejected] main -> main (non-fast-forward)\nerror: failed to push some refs\n",
    );
    assert_human(&msg);
    assert!(
        msg.contains("동기화"),
        "앱 안의 다음 행동을 가리켜야 한다: {msg}"
    );
}

#[test]
fn unresolvable_host_is_network_not_permission() {
    let msg =
        friendly_git_error("ssh: Could not resolve hostname nope: Name or service not known\n");
    assert_human(&msg);
    assert!(msg.contains("네트워크"), "got: {msg}");
}

#[test]
fn permission_denied_is_explained() {
    let msg = friendly_git_error("git@github.com: Permission denied (publickey).\n");
    assert_human(&msg);
    assert!(msg.contains("권한"), "got: {msg}");
}

#[test]
fn repository_not_found_is_explained() {
    let msg = friendly_git_error("remote: Repository not found.\n");
    assert_human(&msg);
    assert!(msg.contains("찾을 수 없습니다"), "got: {msg}");
}

#[test]
fn empty_stderr_never_returns_empty() {
    assert!(!friendly_git_error("").trim().is_empty());
    assert!(!friendly_git_error("   \n ").trim().is_empty());
}
