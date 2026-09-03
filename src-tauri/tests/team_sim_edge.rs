//! Team-simulation stress tests: hostile inputs and weird repo states.
//!
//! Simulates a shared team project (bare origin + clones) and drives the app
//! through its public crate API (`git_companion::git::*`, `Target::Local`).
//! Raw `git` is used for scenario setup only.
//!
//! 한때 FIXME(BUG-n)로 문서화했던 버그들은 모두 수정됐다 — 이 파일의 해당
//! 테스트들은 이제 고쳐진 동작을 회귀 테스트로 못 박는다. 아직 남은 설계상
//! 간극은 `FIXME(UX-…)` 로, 일부러 남긴 동작은 "의도된 관대함" 주석으로
//! 구분해 두었다.
use std::fs;
use std::path::Path;
use tempfile::TempDir;

use git_companion::git::merge::{
    base_unpushed_count, complete_merge, conflict_detail, delete_remote_branch,
    list_merged_remote_branches, list_pending_branches, remaining_conflicts, resolve_conflict,
    start_merge, Resolution,
};
use git_companion::git::ops::{
    list_stashes, list_status_with_base, DiffOpts, StashAction, StatusScope,
};
use git_companion::git::status::FileChangeKind;
use git_companion::git::{self, Target};

// ── helpers (setup only — raw git) ─────────────────────────────────────────

fn git_run(dir: &Path, args: &[&str]) -> std::process::Output {
    let mut c = std::process::Command::new("git");
    c.args(args)
        .current_dir(dir)
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8");
    c.output().unwrap()
}

fn init_repo(dir: &Path) {
    git_run(dir, &["init", "-q", "-b", "main"]);
    git_run(dir, &["config", "user.email", "test@x"]);
    git_run(dir, &["config", "user.name", "tester"]);
    git_run(dir, &["config", "commit.gpgsign", "false"]);
}

/// Bare origin + working clone, both on `main`.
fn make_bare_origin() -> (TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git_run(bare.path(), &["init", "--bare", "-q", "-b", "main"]);
    init_repo(work.path());
    (bare, work)
}

fn add_origin_clone(work: &Path, bare: &Path) {
    let url = format!("file://{}", bare.display());
    git_run(work, &["remote", "add", "origin", &url]);
}

/// Write (creating parent dirs) + `git add -A` + commit.
fn seed_commit(work: &Path, file: &str, body: &str, msg: &str) {
    let p = work.join(file);
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&p, body).unwrap();
    git_run(work, &["add", "-A"]);
    git_run(work, &["commit", "-q", "-m", msg]);
}

/// Standard fixture: origin with `main` containing one pushed commit.
fn team_repo() -> (TempDir, TempDir, Target) {
    let (bare, work) = make_bare_origin();
    add_origin_clone(work.path(), bare.path());
    seed_commit(work.path(), "app.txt", "v1\n", "init");
    git_run(work.path(), &["push", "-q", "-u", "origin", "main"]);
    let target = Target::Local(work.path().into());
    (bare, work, target)
}

fn untracked_paths(target: &Target) -> Vec<String> {
    git::list_status(target)
        .unwrap()
        .files
        .into_iter()
        .filter(|f| f.kind == FileChangeKind::Untracked)
        .map(|f| f.path)
        .collect()
}

// ═══ 1. Korean & special file paths ════════════════════════════════════════

/// Korean filenames and spaces round-trip cleanly through status → add →
/// commit → diff → changed_files. quotepath=off keeps them raw (no \uXXXX,
/// no octal escapes).
#[test]
fn korean_and_space_paths_status_add_commit_diff() {
    let (_bare, work, target) = team_repo();
    // Pre-seed the directories so untracked files are listed individually
    // (untracked-files=normal collapses brand-new dirs to "dir/").
    seed_commit(work.path(), "문서/.keep", "", "dirs");
    seed_commit(work.path(), "src/.keep", "", "dirs2");

    fs::write(work.path().join("문서/회의록 2026.md"), "회의 내용\n").unwrap();
    fs::write(work.path().join("src/한글 파일.ts"), "let x = 1\n").unwrap();

    let untracked = untracked_paths(&target);
    assert!(
        untracked.contains(&"문서/회의록 2026.md".to_string()),
        "Korean path with space must be verbatim, got {untracked:?}"
    );
    assert!(untracked.contains(&"src/한글 파일.ts".to_string()));
    for p in &untracked {
        assert!(!p.contains("\\u") && !p.contains("\\355"), "mangled: {p}");
    }

    // add (explicit paths) + commit through the app API.
    git::add(
        &target,
        &["문서/회의록 2026.md".into(), "src/한글 파일.ts".into()],
    )
    .unwrap();
    let c = git::commit(&target, "한글 파일 추가", false).unwrap();
    assert!(c.ok, "{}", c.message);

    // Modify + diff with a Korean pathspec.
    fs::write(work.path().join("src/한글 파일.ts"), "let x = 2\n").unwrap();
    let d = git::diff(
        &target,
        DiffOpts {
            pathspec: Some("src/한글 파일.ts".into()),
            staged: false,
            stat: false,
        },
    )
    .unwrap();
    assert!(d.contains("let x = 2"), "diff must match Korean pathspec: {d}");

    let changed = git::changed_files(&target, StatusScope::Unstaged).unwrap();
    assert!(changed.iter().any(|f| f.path == "src/한글 파일.ts"));
}

/// A conflict on a Korean-and-space path flows through start_merge →
/// conflict_detail → resolve_conflict → complete_merge, and the path stays
/// verbatim in pending-branch changed_files.
#[test]
fn korean_space_path_conflict_detail_and_resolve() {
    let (_bare, work, target) = team_repo();
    seed_commit(work.path(), "문서/회의록 2026.md", "base\n", "add doc");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/doc"]);
    seed_commit(work.path(), "문서/회의록 2026.md", "feature edit\n", "feat");
    git_run(work.path(), &["push", "-q", "origin", "feature/doc"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    seed_commit(work.path(), "문서/회의록 2026.md", "main edit\n", "main edit");

    let pending = list_pending_branches(&target, "origin", "main").unwrap();
    let b = pending
        .iter()
        .find(|b| b.short_name == "feature/doc")
        .expect("pending branch listed");
    assert!(
        b.changed_files
            .iter()
            .any(|c| c.path == "문서/회의록 2026.md"),
        "changed_files must carry the raw Korean path: {:?}",
        b.changed_files
    );

    let outcome = start_merge(&target, "origin/feature/doc", "main", "origin", None).unwrap();
    assert!(outcome.conflicted);
    assert!(
        outcome
            .conflicted_files
            .contains(&"문서/회의록 2026.md".to_string()),
        "got {:?}",
        outcome.conflicted_files
    );

    let detail = conflict_detail(&target, "문서/회의록 2026.md").unwrap();
    assert!(detail.ours.contains("main edit"));
    assert!(detail.theirs.contains("feature edit"));

    let remaining = resolve_conflict(
        &target,
        "문서/회의록 2026.md",
        &Resolution::Manual {
            content: "merged 병합본\n".into(),
        },
    )
    .unwrap();
    assert!(remaining.is_empty());
    let done = complete_merge(&target, Some("feature/doc 브렌치 병합")).unwrap();
    assert!(done.ok, "{}", done.message);
    let body = fs::read_to_string(work.path().join("문서/회의록 2026.md")).unwrap();
    assert!(body.contains("병합본"));
}

/// 회귀(BUG-1 수정): `"` 가 든 경로는 porcelain v2 / diff --name-status 가
/// C-quoting(`"a\"b.txt"`)으로 감싸서 내보내지만, 이제 반환 전에 언쿼트해
/// 원래 문자열 그대로 돌려준다 — 앱이 돌려준 경로로 하는 diff/add 가
/// 그대로 동작한다(라운드트립 보장).
#[test]
fn quoted_filename_roundtrips_unquoted() {
    let (_bare, work, target) = team_repo();
    fs::write(work.path().join("a\"b.txt"), "v1\n").unwrap();

    let untracked = untracked_paths(&target);
    assert!(
        untracked.contains(&"a\"b.txt".to_string()),
        "quote path must come back verbatim (un-C-quoted), got {untracked:?}"
    );
    assert!(
        !untracked.contains(&"\"a\\\"b.txt\"".to_string()),
        "no C-quoted leftovers: {untracked:?}"
    );

    // Round-trip: diff/add with the path the app itself returned works.
    seed_commit(work.path(), "a\"b.txt", "v1\n", "add quoted");
    fs::write(work.path().join("a\"b.txt"), "v2\n").unwrap();
    let returned = untracked
        .iter()
        .find(|p| p.as_str() == "a\"b.txt")
        .unwrap()
        .clone();
    let via_returned = git::diff(
        &target,
        DiffOpts {
            pathspec: Some(returned.clone()),
            staged: false,
            stat: false,
        },
    )
    .unwrap();
    assert!(
        via_returned.contains("v2"),
        "diff with the returned path must match the file: {via_returned}"
    );
    git::add(&target, &[returned]).unwrap();
    let st = git::list_status(&target).unwrap();
    let f = st.files.iter().find(|f| f.path == "a\"b.txt").unwrap();
    assert!(f.staged, "add with the returned path stages the file");
    git_run(work.path(), &["reset", "-q"]);

    // Pending-branch changed_files (diff --name-status) is unquoted too.
    git_run(work.path(), &["checkout", "-q", "--", "a\"b.txt"]);
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "feature/q"]);
    seed_commit(work.path(), "a\"b.txt", "vq\n", "quoted edit");
    git_run(work.path(), &["push", "-q", "origin", "feature/q"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    let pending = list_pending_branches(&target, "origin", "main").unwrap();
    let b = pending.iter().find(|b| b.short_name == "feature/q").unwrap();
    assert!(
        b.changed_files.iter().any(|c| c.path == "a\"b.txt"),
        "changed_files carries the raw path: {:?}",
        b.changed_files
    );
    assert!(
        b.changed_files.iter().all(|c| !c.path.starts_with('"')),
        "no C-quoted changed_files: {:?}",
        b.changed_files
    );
}

/// 회귀(BUG-2 수정): `ops::add` 는 이제 pathspec 앞에 `--` 를 넣어 `-` 로
/// 시작하는 파일명이 옵션으로 오해되지 않고, git 이 실패하면 Ok 로 삼키는
/// 대신 Err("스테이징 실패: …") 를 돌려준다.
#[test]
fn leading_dash_file_add_stages_and_failures_surface() {
    let (_bare, work, target) = team_repo();
    fs::write(work.path().join("-weird.txt"), "dash\n").unwrap();

    // `--` 덕분에 대시 파일도 명시 경로로 스테이징된다.
    git::add(&target, &["-weird.txt".into()]).unwrap();
    let st = git::list_status(&target).unwrap();
    let f = st.files.iter().find(|f| f.path == "-weird.txt").unwrap();
    assert!(f.staged, "explicit add stages the dash file");
    let untracked = untracked_paths(&target);
    assert!(
        !untracked.contains(&"-weird.txt".to_string()),
        "no longer untracked after add: {untracked:?}"
    );

    // git 이 실패하면(없는 경로) 조용히 넘어가지 않고 Err 가 난다.
    let err = git::add(&target, &["no-such-file.txt".into()])
        .expect_err("adding a nonexistent path must fail loudly");
    assert!(
        format!("{err}").contains("스테이징 실패"),
        "got: {err}"
    );

    // diff/pathspec 도 `--` 를 지나므로 안전하다.
    let d = git::diff(
        &target,
        DiffOpts {
            pathspec: Some("-weird.txt".into()),
            staged: true,
            stat: false,
        },
    )
    .unwrap();
    assert!(d.contains("dash"), "diff -- -weird.txt works: {d}");

    let c = git::commit(&target, "dash file", false).unwrap();
    assert!(c.ok, "{}", c.message);
}

/// 회귀(BUG-3 수정): porcelain v2 rename 라인(`2 …`)의 추가 `<X><score>`
/// 필드를 이제 제대로 소비한다 — 경로에 "R100 " 점수가 섞이지 않고
/// 깨끗한 NEW 경로만 온다(kind 는 Renamed 유지).
#[test]
fn renamed_file_status_path_is_clean_new_path() {
    let (_bare, work, target) = team_repo();
    seed_commit(work.path(), "old.txt", "content\n", "add old");
    git_run(work.path(), &["mv", "old.txt", "b c.txt"]);

    let st = git::list_status(&target).unwrap();
    let renamed: Vec<_> = st
        .files
        .iter()
        .filter(|f| f.kind == FileChangeKind::Renamed)
        .collect();
    assert_eq!(renamed.len(), 1);
    assert_eq!(
        renamed[0].path, "b c.txt",
        "score field consumed — clean new path only"
    );
    assert!(renamed[0].staged, "git mv stages the rename");
}

/// 회귀(BUG-4 수정): porcelain v2 unmerged 라인(`u …`)은 스테이지 해시가
/// 3개라 필드가 하나 더 많다 — 이제 splitn(11) 로 잘라 충돌 파일 경로에
/// stage-3 SHA 40자가 붙지 않는다.
#[test]
fn conflicted_file_status_path_is_clean() {
    let (_bare, work, target) = team_repo();
    git_run(work.path(), &["checkout", "-q", "-b", "side"]);
    seed_commit(work.path(), "app.txt", "side\n", "side edit");
    git_run(work.path(), &["checkout", "-q", "main"]);
    seed_commit(work.path(), "app.txt", "main\n", "main edit");
    let m = git_run(work.path(), &["merge", "side"]);
    assert!(!m.status.success(), "merge must conflict");

    let st = git::list_status(&target).unwrap();
    let conflicted: Vec<_> = st
        .files
        .iter()
        .filter(|f| f.kind == FileChangeKind::Conflicted)
        .collect();
    assert_eq!(conflicted.len(), 1);
    assert_eq!(
        conflicted[0].path, "app.txt",
        "no stage-hash prefix on the conflicted path"
    );
}

// ═══ 2. Korean & slashed branch names ══════════════════════════════════════

/// Full lifecycle of a Korean slashed branch through the app API: create,
/// commit, push (HEAD:refspec), pending list short_name, merge with the
/// "<short> 브렌치 병합" message, base_unpushed_count, remote delete.
#[test]
fn korean_slashed_branch_full_lifecycle() {
    let (_bare, work, target) = team_repo();

    git::create_branch(&target, "기능/로그인-개선").unwrap();
    seed_commit(work.path(), "login.ts", "로그인\n", "로그인 개선");
    let p = git::push(&target, Some("기능/로그인-개선"), None).unwrap();
    assert!(p.ok, "push Korean branch: {}", p.message);

    git_run(work.path(), &["checkout", "-q", "main"]);
    let pending = list_pending_branches(&target, "origin", "main").unwrap();
    let b = pending
        .iter()
        .find(|b| b.short_name == "기능/로그인-개선")
        .expect("Korean short_name computed from origin/기능/로그인-개선");
    assert_eq!(b.name, "origin/기능/로그인-개선");
    assert_eq!(b.ahead, 1);
    assert!(b.changed_files.iter().any(|c| c.path == "login.ts"));

    let outcome = start_merge(&target, "origin/기능/로그인-개선", "main", "origin", None).unwrap();
    assert!(outcome.ok, "{}", outcome.message);
    let commits = git::list_commits(&target, "main", 1).unwrap();
    assert_eq!(
        commits[0].message, "기능/로그인-개선 브렌치 병합",
        "team-convention merge message with the Korean short name"
    );

    // Merge commit + feature commit not on origin/main yet.
    assert_eq!(base_unpushed_count(&target, "origin", "main").unwrap(), 2);

    let p = git::push(&target, Some("main"), None).unwrap();
    assert!(p.ok, "{}", p.message);
    assert_eq!(base_unpushed_count(&target, "origin", "main").unwrap(), 0);

    delete_remote_branch(&target, "origin", "main", "기능/로그인-개선").unwrap();
    let ls = git_run(
        work.path(),
        &["ls-remote", "--heads", "origin", "기능/로그인-개선"],
    );
    assert!(
        String::from_utf8_lossy(&ls.stdout).trim().is_empty(),
        "Korean remote branch deleted"
    );
}

/// Branch names with `#` and deep slashes survive create/push/checkout and
/// short_name computation.
#[test]
fn hash_and_deep_slash_branch_names() {
    let (_bare, work, target) = team_repo();

    // `#` in the name (never goes through a shell locally).
    git::create_branch(&target, "bugfix/UI-#123").unwrap();
    let cur = git_run(work.path(), &["branch", "--show-current"]);
    assert_eq!(String::from_utf8_lossy(&cur.stdout).trim(), "bugfix/UI-#123");
    seed_commit(work.path(), "ui.txt", "fix\n", "ui fix");
    let p = git::push(&target, Some("bugfix/UI-#123"), None).unwrap();
    assert!(p.ok, "{}", p.message);
    let ls = git_run(work.path(), &["ls-remote", "--heads", "origin"]);
    assert!(
        String::from_utf8_lossy(&ls.stdout).contains("refs/heads/bugfix/UI-#123"),
        "hash branch on remote"
    );

    // Deep a/b/c/d.
    git_run(work.path(), &["checkout", "-q", "main"]);
    git::create_branch(&target, "a/b/c/d").unwrap();
    seed_commit(work.path(), "deep.txt", "d\n", "deep");
    let p = git::push(&target, Some("a/b/c/d"), None).unwrap();
    assert!(p.ok, "{}", p.message);
    git_run(work.path(), &["checkout", "-q", "main"]);

    let pending = list_pending_branches(&target, "origin", "main").unwrap();
    let names: Vec<&str> = pending.iter().map(|b| b.short_name.as_str()).collect();
    assert!(names.contains(&"a/b/c/d"), "got {names:?}");
    assert!(names.contains(&"bugfix/UI-#123"), "got {names:?}");

    // checkout_branch resolves the deep name (and the origin/ prefixed form).
    git::checkout_branch(&target, "a/b/c/d").unwrap();
    let cur = git_run(work.path(), &["branch", "--show-current"]);
    assert_eq!(String::from_utf8_lossy(&cur.stdout).trim(), "a/b/c/d");
    git::checkout_branch(&target, "origin/bugfix/UI-#123").unwrap();
    let cur = git_run(work.path(), &["branch", "--show-current"]);
    assert_eq!(
        String::from_utf8_lossy(&cur.stdout).trim(),
        "bugfix/UI-#123"
    );
}

/// Branches whose names share the base as a prefix (`main-hotfix`,
/// `mainline`) must not be confused with `main` anywhere (prefix matching
/// uses exact "<remote>/<base>" comparisons — verified here).
#[test]
fn base_prefix_branch_names_are_not_confused_with_base() {
    let (_bare, work, target) = team_repo();

    git_run(work.path(), &["checkout", "-q", "-b", "main-hotfix"]);
    seed_commit(work.path(), "hotfix.txt", "h\n", "hotfix");
    git_run(work.path(), &["push", "-q", "origin", "main-hotfix"]);
    git_run(work.path(), &["checkout", "-q", "main"]);
    git_run(work.path(), &["checkout", "-q", "-b", "mainline"]);
    seed_commit(work.path(), "line.txt", "l\n", "mainline work");
    git_run(work.path(), &["push", "-q", "origin", "mainline"]);
    git_run(work.path(), &["checkout", "-q", "main"]);

    let pending = list_pending_branches(&target, "origin", "main").unwrap();
    let names: Vec<&str> = pending.iter().map(|b| b.short_name.as_str()).collect();
    assert!(names.contains(&"main-hotfix"), "got {names:?}");
    assert!(names.contains(&"mainline"), "got {names:?}");
    assert!(!names.contains(&"main"), "base never listed: {names:?}");
    // short_name must not be a stripped remainder like "-hotfix" or "line".
    assert!(!names.contains(&"-hotfix") && !names.contains(&"line"));

    // behind_base from a prefix-named branch compares against origin/main,
    // not origin/mainline.
    git_run(work.path(), &["checkout", "-q", "mainline"]);
    let st = list_status_with_base(&target, "main").unwrap();
    assert_eq!(st.behind_base, 0, "mainline == main + 1, not behind");
    git_run(work.path(), &["checkout", "-q", "main"]);

    // Merge main-hotfix, push, then cleanup rules:
    let out = start_merge(&target, "origin/main-hotfix", "main", "origin", None).unwrap();
    assert!(out.ok, "{}", out.message);
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["fetch", "-q", "origin"]);

    let merged = list_merged_remote_branches(&target, "origin", "main").unwrap();
    let mnames: Vec<&str> = merged.iter().map(|b| b.short_name.as_str()).collect();
    assert!(mnames.contains(&"main-hotfix"), "got {mnames:?}");
    assert!(!mnames.contains(&"main"), "base excluded: {mnames:?}");
    assert!(!mnames.contains(&"mainline"), "unmerged excluded: {mnames:?}");

    assert!(
        delete_remote_branch(&target, "origin", "main", "main").is_err(),
        "deleting the base is refused"
    );
    assert!(
        delete_remote_branch(&target, "origin", "main", "mainline").is_err(),
        "unmerged prefix-named branch refused"
    );
    delete_remote_branch(&target, "origin", "main", "main-hotfix").unwrap();
    let ls = git_run(work.path(), &["ls-remote", "--heads", "origin"]);
    let heads = String::from_utf8_lossy(&ls.stdout);
    assert!(!heads.contains("refs/heads/main-hotfix"));
    assert!(heads.contains("refs/heads/main"), "main survives the delete");
    assert!(heads.contains("refs/heads/mainline"), "mainline survives");
}

/// 회귀(BUG-5 수정): `create_branch` 는 이제 git 종료 코드를 확인한다 —
/// 잘못된 이름("bad name")은 "쓸 수 없는 브랜치 이름" Err 로, 그 밖의
/// 실패(이미 있는 이름 등)는 "브랜치 생성 실패: …" Err 로 표면화된다.
#[test]
fn create_branch_invalid_name_and_other_failures_error() {
    let (_bare, work, target) = team_repo();

    let err = git::create_branch(&target, "bad name")
        .expect_err("space in a branch name must be rejected loudly");
    assert!(
        format!("{err}").contains("쓸 수 없는 브랜치 이름"),
        "got: {err}"
    );
    let cur = git_run(work.path(), &["branch", "--show-current"]);
    assert_eq!(
        String::from_utf8_lossy(&cur.stdout).trim(),
        "main",
        "still on main — nothing half-created"
    );

    // 이름 규칙 외의 실패(이미 존재하는 브랜치)는 일반 실패 접두사로.
    let err = git::create_branch(&target, "main")
        .expect_err("creating an existing branch must fail");
    assert!(
        format!("{err}").contains("브랜치 생성 실패"),
        "got: {err}"
    );
}

// ═══ 3. Commit message edge cases ══════════════════════════════════════════

/// Hostile commit messages round-trip cleanly: multi-line (subject = first
/// line), quotes + `$(rm -rf /)` literal (argv-passing, no shell → no
/// injection), emoji, 1000-char subject.
#[test]
fn commit_message_hostile_inputs_roundtrip() {
    let (_bare, work, target) = team_repo();

    let long_subject = "가".repeat(500) + &"x".repeat(500);
    let cases: Vec<(&str, String)> = vec![
        (
            "multiline",
            "제목: 첫 줄만 subject\n\n본문 첫 줄\n본문 둘째 줄".to_string(),
        ),
        (
            "quotes-and-subshell",
            r#"say "hello" && $(rm -rf /) `touch /tmp/pwned` ; rm -rf ~"#.to_string(),
        ),
        ("emoji", "🚀 배포 완료 ✨ (feat: 로그인)".to_string()),
        ("long", long_subject.clone()),
    ];

    for (i, (name, msg)) in cases.iter().enumerate() {
        fs::write(work.path().join("f.txt"), format!("v{i}\n")).unwrap();
        let c = git::commit(&target, msg, true).unwrap();
        assert!(c.ok, "{name}: {}", c.message);
        let head = git::list_commits(&target, "main", 1).unwrap();
        let expected_subject = msg.lines().next().unwrap();
        assert_eq!(
            head[0].message, expected_subject,
            "{name}: subject round-trip"
        );
    }

    // No injection happened: the repo (and its parent tempdir) still exist,
    // and the subshell text is stored verbatim.
    assert!(work.path().join("app.txt").exists());
    let commits = git::list_commits(&target, "main", 10).unwrap();
    assert!(commits
        .iter()
        .any(|c| c.message.contains("$(rm -rf /)") && c.message.contains("`touch /tmp/pwned`")));
    assert!(commits.iter().any(|c| c.message == long_subject));
    assert!(!Path::new("/tmp/pwned").exists());
}

/// 회귀(BUG-6 수정): 커밋 제목 속의 0x1f(unit separator) 가 parse_log 의
/// 필드 구분자와 겹쳐도 페이지 전체가 죽지 않는다 — sha 는 앞에서, 나머지
/// 필드는 뒤에서 세고 가운데를 제목으로 되붙여 그대로 돌려준다.
#[test]
fn commit_subject_with_unit_separator_parses_whole_page() {
    let (_bare, work, target) = team_repo();
    fs::write(work.path().join("f.txt"), "x\n").unwrap();
    let msg = "bad\u{1f}subject";
    let c = git::commit(&target, msg, true).unwrap();
    assert!(c.ok, "the commit itself succeeds: {}", c.message);

    let commits = git::list_commits(&target, "main", 5).unwrap();
    assert_eq!(commits.len(), 2, "the whole page parses, not just one line");
    assert_eq!(
        commits[0].message, msg,
        "raw subject preserved (extra separators re-joined)"
    );
    assert_eq!(commits[0].author, "tester");
    assert_eq!(
        commits[0].parents.len(),
        1,
        "parent field not eaten by the split: {:?}",
        commits[0].parents
    );
    assert_eq!(commits[1].message, "init", "neighboring commits intact");
}

// ═══ 4. Empty / degenerate repos ═══════════════════════════════════════════

/// A repo with zero commits: status reports the unborn branch name, branch
/// and commit listings are empty (not errors), and the first commit works.
#[test]
fn zero_commit_repo_is_handled_gracefully() {
    let td = TempDir::new().unwrap();
    init_repo(td.path());
    fs::write(td.path().join("x.txt"), "x\n").unwrap();
    let target = Target::Local(td.path().into());

    let st = git::list_status(&target).unwrap();
    assert_eq!(
        st.branch.as_deref(),
        Some("main"),
        "unborn branch still has a name"
    );
    assert_eq!(st.ahead, 0);
    assert!(st
        .files
        .iter()
        .any(|f| f.path == "x.txt" && f.kind == FileChangeKind::Untracked));

    let branches = git::list_branches(td.path()).unwrap();
    assert!(branches.is_empty(), "no refs yet: {branches:?}");

    let commits = git::list_commits(&target, "main", 10).unwrap();
    assert!(commits.is_empty(), "no commits → empty page, not an error");

    let c = git::commit(&target, "첫 커밋", true).unwrap();
    assert!(c.ok, "{}", c.message);
    assert!(c.sha.is_some());
}

/// A repo with no origin: push/pull/sync_to_base give friendly Korean
/// errors; fetch_origin stays lenient by design (offline sync).
#[test]
fn no_origin_repo_errors_are_friendly() {
    let td = TempDir::new().unwrap();
    init_repo(td.path());
    fs::write(td.path().join("a.txt"), "a\n").unwrap();
    git_run(td.path(), &["add", "-A"]);
    git_run(td.path(), &["commit", "-q", "-m", "init"]);
    let target = Target::Local(td.path().into());

    let p = git::push(&target, None, None).unwrap();
    assert!(!p.ok);
    assert!(
        p.message.contains("원격(origin)이 없어서"),
        "friendly no-remote push message, got: {}",
        p.message
    );

    let pl = git::pull(&target).unwrap();
    assert!(!pl.ok);
    assert!(pl.conflicted_files.is_empty());
    assert!(
        pl.message.contains("원격(origin)이 없어서"),
        "friendly no-remote pull message, got: {}",
        pl.message
    );

    // 회귀(UX-1 수정): sync_to_base 의 병합 실패는 friendly_git_error 를
    // 거친다 — 영어 "not something we can merge" 대신 한글 안내가 뜬다.
    let err = git::sync_to_base(&target, "main", "origin").unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("병합할 대상을 찾을 수 없습니다"),
        "friendly no-merge-target message, got: {msg}"
    );
    assert!(
        !msg.contains("not something we can merge"),
        "raw English error must not leak: {msg}"
    );

    // BUG-7 아님 — 의도된 관대함: fetch_origin 은 일부러 종료 코드를 따지지
    // 않는다(오프라인에서도 동기화 흐름이 계속 굴러가야 해서, 실패는
    // best-effort 로 삼키고 stderr 만 돌려준다). 원격이 없어도 Ok.
    let fetched = git::fetch_origin(td.path()).unwrap();
    assert!(
        fetched.contains("does not appear to be a git repository")
            || fetched.contains("No such remote"),
        "lenient Ok(stderr) so offline callers can proceed: {fetched}"
    );
}

/// Detached HEAD: status reports branch=None; push/pull are refused up front
/// with an accurate "브랜치 위에 있지 않습니다" 안내(회귀, UX-2 수정 — 예전엔
/// HEAD:HEAD 푸시를 시도하고 "원격에 새 변경" 이라는 엉뚱한 메시지를 냈다);
/// pending list works.
#[test]
fn detached_head_status_push_pull_refused_with_accurate_message() {
    let (bare, work, target) = team_repo();
    seed_commit(work.path(), "app.txt", "v2\n", "second");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    let c1 = git_run(work.path(), &["rev-parse", "HEAD~1"]);
    let c1 = String::from_utf8_lossy(&c1.stdout).trim().to_string();
    git_run(work.path(), &["checkout", "-q", &c1]);

    let st = git::list_status(&target).unwrap();
    assert_eq!(st.branch, None, "detached → branch=None, no panic");

    let before = git_run(bare.path(), &["rev-parse", "main"]);
    let before = String::from_utf8_lossy(&before.stdout).trim().to_string();

    // push(None) 은 branch=="HEAD" 를 먼저 감지해 원격을 건드리기 전에 Err.
    let err = git::push(&target, None, None)
        .expect_err("detached push must be refused");
    assert!(
        format!("{err}").contains("브랜치로 전환한 뒤 푸시하세요"),
        "accurate detached-HEAD advice, got: {err}"
    );
    let after = git_run(bare.path(), &["rev-parse", "main"]);
    let after = String::from_utf8_lossy(&after.stdout).trim().to_string();
    assert_eq!(before, after, "remote main must not move from a detached push");

    // pull 도 같은 가드를 지난다.
    let err = git::pull(&target).expect_err("detached pull must be refused");
    assert!(
        format!("{err}").contains("받아오세요"),
        "accurate detached-HEAD pull advice, got: {err}"
    );

    let pending = list_pending_branches(&target, "origin", "main").unwrap();
    assert!(
        pending.iter().all(|b| b.short_name != "HEAD"),
        "no phantom HEAD entry: {pending:?}"
    );
}

/// 회귀(BUG-8 수정): detached HEAD 에서 sync_to_base 는 병합을 시작하기
/// 전에 거부한다 — 예전에는 어느 브랜치에도 속하지 않는 병합 커밋을 만들어
/// 다음 checkout 과 함께 미아로 만들었다(진행 중 병합 가드와 같은 결의
/// `current == "HEAD"` 가드).
#[test]
fn detached_head_sync_is_refused_before_merging() {
    let (_bare, work, target) = team_repo();
    seed_commit(work.path(), "app.txt", "v2\n", "second");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    let c1 = git_run(work.path(), &["rev-parse", "HEAD~1"]);
    let c1 = String::from_utf8_lossy(&c1.stdout).trim().to_string();
    git_run(work.path(), &["checkout", "-q", &c1]);

    let err = git::sync_to_base(&target, "main", "origin")
        .expect_err("detached sync must be refused");
    assert!(
        format!("{err}").contains("지금 브랜치 위에 있지 않습니다(detached HEAD)"),
        "got: {err}"
    );

    // 병합 커밋이 만들어지지 않았다 — HEAD 는 체크아웃한 커밋 그대로.
    let head = git_run(work.path(), &["rev-parse", "HEAD"]);
    assert_eq!(
        String::from_utf8_lossy(&head.stdout).trim(),
        c1,
        "HEAD untouched, no stranded merge commit"
    );
    let cur = git_run(work.path(), &["branch", "--show-current"]);
    assert_eq!(
        String::from_utf8_lossy(&cur.stdout).trim(),
        "",
        "still detached"
    );
}

// ═══ 5. Weird worktree states ══════════════════════════════════════════════

/// Staged deletion + same-name untracked recreate, and file→directory
/// replacement: list_status reports both sides without panicking, and a
/// stage-all commit gets the tree back to consistent.
#[test]
fn staged_delete_recreate_and_file_to_dir_survive_status_and_commit() {
    let (_bare, work, target) = team_repo();
    seed_commit(work.path(), "f.txt", "old\n", "add f");
    seed_commit(work.path(), "d.txt", "file\n", "add d");

    // f.txt: staged deletion, then an untracked file reappears at the path.
    git_run(work.path(), &["rm", "-q", "f.txt"]);
    fs::write(work.path().join("f.txt"), "reborn\n").unwrap();
    // d.txt: the file becomes a directory.
    fs::remove_file(work.path().join("d.txt")).unwrap();
    fs::create_dir(work.path().join("d.txt")).unwrap();
    fs::write(work.path().join("d.txt/inner.txt"), "inner\n").unwrap();

    let st = git::list_status(&target).unwrap();
    let f_entries: Vec<_> = st.files.iter().filter(|f| f.path == "f.txt").collect();
    assert_eq!(
        f_entries.len(),
        2,
        "staged deletion AND untracked recreate both listed: {:?}",
        st.files
    );
    assert!(f_entries
        .iter()
        .any(|f| f.kind == FileChangeKind::Deleted && f.staged));
    assert!(f_entries
        .iter()
        .any(|f| f.kind == FileChangeKind::Untracked));
    // 회귀(BUG-10 수정): d.txt 는 워크트리에서만 지워졌다(XY = ".D") —
    // `classify` 가 이제 인덱스(X) 열이 '.' 이면 워크트리(Y) 열로 분류하므로
    // 미스테이지 삭제가 Modified 가 아닌 Deleted 로 나온다. (git 은 그 경로의
    // 추적 파일이 인덱스에 남아 있는 동안 미추적 d.txt/ 내용물을 숨기므로,
    // 교체를 나타내는 항목은 이 하나뿐이다.)
    let d = st
        .files
        .iter()
        .find(|f| f.path == "d.txt")
        .expect("file→dir replacement listed");
    assert_eq!(
        d.kind,
        FileChangeKind::Deleted,
        "worktree-only deletion classified by the Y column"
    );
    assert!(!d.staged && d.unstaged);

    let c = git::commit(&target, "weird tree states", true).unwrap();
    assert!(c.ok, "stage-all commit survives: {}", c.message);
    let st = git::list_status(&target).unwrap();
    assert!(st.files.is_empty(), "clean after commit: {:?}", st.files);
    assert_eq!(
        fs::read_to_string(work.path().join("f.txt")).unwrap(),
        "reborn\n"
    );
}

/// Symlink, executable-bit flip, and a submodule: status classifies them
/// without panicking and a stage-all commit succeeds.
#[test]
fn symlink_execbit_and_submodule_status_and_commit() {
    let (bare, work, target) = team_repo();
    seed_commit(work.path(), "script.sh", "#!/bin/sh\necho hi\n", "script");

    // Exec bit flip only (content unchanged).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            work.path().join("script.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    // Symlink.
    #[cfg(unix)]
    std::os::unix::fs::symlink("script.sh", work.path().join("link.sh")).unwrap();
    // Submodule (raw setup; file protocol must be allowed explicitly).
    let sub_url = bare.path().display().to_string();
    let out = git_run(
        work.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &sub_url,
            "vendor/sub",
        ],
    );
    assert!(
        out.status.success(),
        "submodule add: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let st = git::list_status(&target).unwrap();
    let mode_flip = st.files.iter().find(|f| f.path == "script.sh");
    assert!(
        matches!(
            mode_flip,
            Some(f) if f.kind == FileChangeKind::Modified && f.unstaged
        ),
        "exec-bit-only flip shows as unstaged modification: {st:?}"
    );
    assert!(st
        .files
        .iter()
        .any(|f| f.path == "link.sh" && f.kind == FileChangeKind::Untracked));
    assert!(
        st.files
            .iter()
            .any(|f| f.path == "vendor/sub" && f.kind == FileChangeKind::Added && f.staged),
        "submodule entry parsed: {:?}",
        st.files
    );
    assert!(st.files.iter().any(|f| f.path == ".gitmodules"));

    let c = git::commit(&target, "링크/모드/서브모듈", true).unwrap();
    assert!(c.ok, "{}", c.message);
    let st = git::list_status(&target).unwrap();
    assert!(st.files.is_empty(), "clean after commit: {:?}", st.files);
}

/// 회귀(UX-3/BUG-9 수정): stash Save 는 이제 `-u` 로 미추적 파일까지
/// 보관한다 — 새 파일만 있어도 조용한 no-op 이 아니라 실제로 스태시되고,
/// 정말 보관할 것이 없는 깨끗한 트리에서는 "보관할 변경이 없습니다." Err.
#[test]
fn stash_with_untracked_only_changes_actually_stashes() {
    let (_bare, work, target) = team_repo();

    // 깨끗한 트리 → 성공을 가장하지 않고 Err.
    let err = git::stash(
        &target,
        StashAction::Save {
            message: Some("빈 저장".into()),
        },
    )
    .expect_err("clean tree must not pretend to save");
    assert!(
        format!("{err}").contains("보관할 변경이 없습니다"),
        "got: {err}"
    );

    // 미추적 파일만 있어도 `-u` 덕분에 보관된다.
    fs::write(work.path().join("brand-new.txt"), "wip\n").unwrap();
    git::stash(
        &target,
        StashAction::Save {
            message: Some("임시 저장".into()),
        },
    )
    .unwrap();

    let stashes = list_stashes(&target).unwrap();
    assert_eq!(stashes.len(), 1, "the untracked file WAS stashed");
    assert!(
        !work.path().join("brand-new.txt").exists(),
        "tree parked clean"
    );
    assert!(untracked_paths(&target).is_empty());

    // Pop 으로 그대로 되돌아온다.
    git::stash(&target, StashAction::Pop).unwrap();
    assert!(list_stashes(&target).unwrap().is_empty());
    let untracked = untracked_paths(&target);
    assert!(untracked.contains(&"brand-new.txt".to_string()));
}

/// 회귀(BUG-9 수정): 충돌한 `git stash pop`(exit 1) 을 더는 Ok 로 삼키지
/// 않는다 — "스태시를 복원하다 충돌" Err 를 돌려주고, git 규칙대로 스태시
/// 항목은 지워지지 않고 남는다.
#[test]
fn stash_pop_conflict_errors_and_keeps_entry() {
    let (_bare, work, target) = team_repo();

    fs::write(work.path().join("app.txt"), "stashed edit\n").unwrap();
    git::stash(
        &target,
        StashAction::Save {
            message: Some("충돌 예정".into()),
        },
    )
    .unwrap();
    seed_commit(work.path(), "app.txt", "committed edit\n", "conflicting");

    let err = git::stash(&target, StashAction::Pop)
        .expect_err("conflicted pop must not report success");
    assert!(
        format!("{err}").contains("스태시를 복원하다 충돌"),
        "got: {err}"
    );

    let remaining = remaining_conflicts(&target).unwrap();
    assert_eq!(
        remaining,
        vec!["app.txt".to_string()],
        "worktree IS in conflict"
    );
    assert_eq!(
        list_stashes(&target).unwrap().len(),
        1,
        "git keeps the stash entry on a conflicted pop"
    );
    let body = fs::read_to_string(work.path().join("app.txt")).unwrap();
    assert!(body.contains("<<<<<<<"), "markers in the file: {body}");
}

// ═══ 6. Huge and binary files ══════════════════════════════════════════════

fn make_conflict(
    work: &Path,
    target: &Target,
    file: &str,
    base: &str,
    ours: &str,
    theirs: &str,
) {
    seed_commit(work, file, base, "base body");
    git_run(work, &["push", "-q", "origin", "main"]);
    git_run(work, &["checkout", "-q", "-b", "feature/big"]);
    seed_commit(work, file, theirs, "their edit");
    git_run(work, &["push", "-q", "origin", "feature/big"]);
    git_run(work, &["checkout", "-q", "main"]);
    seed_commit(work, file, ours, "our edit");
    let outcome = start_merge(target, "origin/feature/big", "main", "origin", None).unwrap();
    assert!(outcome.conflicted, "expected a conflict: {}", outcome.message);
}

/// A 2 MiB text conflict takes the too_large path: bodies are not embedded,
/// but side-picking still resolves it.
#[test]
fn two_mib_text_conflict_reports_too_large_and_resolves() {
    let (_bare, work, target) = team_repo();
    let big = "x".repeat(2 * 1024 * 1024);
    let ours = format!("{big}\nours\n");
    let theirs = format!("{big}\ntheirs\n");
    make_conflict(work.path(), &target, "big.txt", &format!("{big}\nbase\n"), &ours, &theirs);

    let detail = conflict_detail(&target, "big.txt").unwrap();
    assert!(detail.too_large, "2MiB > 1MiB cap");
    assert!(!detail.is_binary);
    assert!(detail.ours.is_empty() && detail.theirs.is_empty());
    assert!(
        detail.working.is_empty(),
        "working copy also over the cap → empty"
    );

    let remaining = resolve_conflict(&target, "big.txt", &Resolution::Ours).unwrap();
    assert!(remaining.is_empty());
    let done = complete_merge(&target, Some("feature/big 브렌치 병합")).unwrap();
    assert!(done.ok, "{}", done.message);
    let body = fs::read_to_string(work.path().join("big.txt")).unwrap();
    assert!(body.ends_with("ours\n"));
}

/// The is_binary heuristic mirrors git: NUL within the first 8000 bytes →
/// binary; a NUL after byte 8000 is NOT detected (contents are returned with
/// the NUL embedded). Both sides of the boundary verified.
#[test]
fn nul_byte_binary_heuristic_boundary() {
    // Early NUL → binary.
    {
        let (_bare, work, target) = team_repo();
        let base = "header\0base tail\n";
        let ours = "header\0ours tail\n";
        let theirs = "header\0theirs tail\n";
        make_conflict(work.path(), &target, "bin.dat", base, ours, theirs);
        let detail = conflict_detail(&target, "bin.dat").unwrap();
        assert!(detail.is_binary, "NUL at byte 6 → binary");
        assert!(detail.ours.is_empty() && detail.theirs.is_empty());
        // Side-picking still works for binary conflicts.
        let remaining = resolve_conflict(&target, "bin.dat", &Resolution::Theirs).unwrap();
        assert!(remaining.is_empty());
    }
    // NUL after 8000 bytes → NOT binary (heuristic boundary, matches git's
    // own buffer_is_binary window; contents round-trip with the NUL inside).
    {
        let (_bare, work, target) = team_repo();
        let prefix = "a".repeat(8100);
        let base = format!("{prefix}\0base\n");
        let ours = format!("{prefix}\0ours\n");
        let theirs = format!("{prefix}\0theirs\n");
        make_conflict(work.path(), &target, "late-nul.dat", &base, &ours, &theirs);
        let detail = conflict_detail(&target, "late-nul.dat").unwrap();
        assert!(
            !detail.is_binary,
            "NUL at byte 8100 is beyond the 8000-byte window"
        );
        assert!(detail.ours.contains('\0') && detail.ours.ends_with("ours\n"));
        assert!(detail.theirs.ends_with("theirs\n"));
    }
}

/// Empty-file vs content conflict: one stage is an empty blob — detail and
/// resolution must not choke on the empty side.
#[test]
fn empty_vs_content_conflict_resolves() {
    let (_bare, work, target) = team_repo();
    make_conflict(
        work.path(),
        &target,
        "note.txt",
        "original content\n",
        "changed content\n",
        "", // theirs truncates the file to empty
    );

    let detail = conflict_detail(&target, "note.txt").unwrap();
    assert!(!detail.is_binary && !detail.too_large);
    assert_eq!(detail.ours, "changed content\n");
    assert_eq!(detail.theirs, "", "empty side comes back as empty string");
    assert_eq!(detail.base.as_deref(), Some("original content\n"));

    let remaining = resolve_conflict(&target, "note.txt", &Resolution::Theirs).unwrap();
    assert!(remaining.is_empty());
    let done = complete_merge(&target, None).unwrap();
    assert!(done.ok, "{}", done.message);
    assert_eq!(
        fs::read_to_string(work.path().join("note.txt")).unwrap(),
        "",
        "resolved to the empty side"
    );
}

// ═══ 7. expand_tilde and paths ═════════════════════════════════════════════

#[test]
fn expand_tilde_variants() {
    let home = std::env::var("HOME").expect("HOME set in test env");

    assert_eq!(git::expand_tilde("~"), home);
    assert_eq!(git::expand_tilde("~/x"), format!("{home}/x"));
    assert_eq!(git::expand_tilde("~/x/"), format!("{home}/x/"));
    assert_eq!(git::expand_tilde("  ~/x  "), format!("{home}/x"), "trimmed");
    assert_eq!(git::expand_tilde("  ~  "), home, "bare ~ trimmed");

    // ~user must NOT be mis-expanded (we don't resolve other users' homes).
    assert_eq!(git::expand_tilde("~user"), "~user");
    assert_eq!(git::expand_tilde("~user/x"), "~user/x");
    assert_eq!(git::expand_tilde("~root/.ssh"), "~root/.ssh");

    // `..` passes through untouched (no canonicalization).
    assert_eq!(git::expand_tilde("~/../etc"), format!("{home}/../etc"));
    assert_eq!(git::expand_tilde("/a/../b"), "/a/../b");
    // Absolute and relative paths untouched.
    assert_eq!(git::expand_tilde("/opt/repo/"), "/opt/repo/");
    assert_eq!(git::expand_tilde("rel/path"), "rel/path");
    // A tilde in the middle is not a home reference.
    assert_eq!(git::expand_tilde("/data/~backup"), "/data/~backup");

    // "~/" — trailing slash: expands into the home dir (join with an empty
    // rest keeps a trailing separator).
    let expanded = git::expand_tilde("~/");
    assert!(
        expanded == home || expanded == format!("{home}/"),
        "~/ expands to the home dir, got {expanded}"
    );
}

// ═══ 8. Upstream oddities ══════════════════════════════════════════════════

/// Upstream with a DIFFERENT name (`push -u origin HEAD:other-name`
/// happened): status ahead/behind track origin/other-name, but the app's
/// push(None) targets origin/<local-name> — documented divergence.
#[test]
fn upstream_with_different_name_status_vs_push_target() {
    let (bare, work, target) = team_repo();
    git_run(work.path(), &["checkout", "-q", "-b", "mywork"]);
    seed_commit(work.path(), "w.txt", "w1\n", "w1");
    git_run(work.path(), &["push", "-q", "-u", "origin", "HEAD:other-name"]);

    let st = git::list_status(&target).unwrap();
    assert_eq!(st.branch.as_deref(), Some("mywork"));
    assert_eq!(
        st.upstream.as_deref(),
        Some("origin/other-name"),
        "upstream is the odd name"
    );
    assert_eq!((st.ahead, st.behind), (0, 0));

    seed_commit(work.path(), "w.txt", "w2\n", "w2");
    let st = git::list_status(&target).unwrap();
    assert_eq!(st.ahead, 1, "ahead counted against origin/other-name");

    let other_before = git_run(bare.path(), &["rev-parse", "other-name"]);
    let other_before = String::from_utf8_lossy(&other_before.stdout).trim().to_string();

    // FIXME(UX-4): the ahead=1 the user just saw was measured against
    // origin/other-name, but push(None) pushes HEAD:mywork — it creates a
    // NEW remote branch and leaves other-name (the branch the numbers were
    // about) untouched. The numbers and the action disagree. Desired: push
    // to the configured upstream when one exists (or surface the mismatch).
    let p = git::push(&target, None, None).unwrap();
    assert!(p.ok, "{}", p.message);
    let ls = git_run(bare.path(), &["rev-parse", "mywork"]);
    assert!(
        ls.status.success(),
        "push created origin/mywork instead of updating other-name"
    );
    let other_after = git_run(bare.path(), &["rev-parse", "other-name"]);
    let other_after = String::from_utf8_lossy(&other_after.stdout).trim().to_string();
    assert_eq!(
        other_before, other_after,
        "origin/other-name did not receive the new commit"
    );
    // push -u re-pointed the upstream to origin/mywork, so numbers are
    // coherent again *after* the push.
    let st = git::list_status(&target).unwrap();
    assert_eq!(st.upstream.as_deref(), Some("origin/mywork"));
    assert_eq!(st.ahead, 0);
}

/// Upstream deleted on the remote (+ prune): porcelain drops the
/// `# branch.ab` line while keeping `# branch.upstream` — 회귀(UX-5 수정):
/// list_status_with_base 가 업스트림 ref 소실을 감지하면 `origin/<base>..HEAD`
/// 커밋 수로 ahead 를 대신 채워, 미푸시 커밋이 "할 일 없음"으로 숨지 않는다.
/// behind_base 는 종전대로 origin/<base> 와 독립 계산.
#[test]
fn upstream_pruned_falls_back_to_base_ahead_count() {
    let (_bare, work, target) = team_repo();
    git_run(work.path(), &["checkout", "-q", "-b", "feat"]);
    seed_commit(work.path(), "feat.txt", "f1\n", "f1");
    git_run(work.path(), &["push", "-q", "-u", "origin", "feat"]);

    // Base moves ahead by one while we're away.
    git_run(work.path(), &["checkout", "-q", "main"]);
    seed_commit(work.path(), "app.txt", "v2\n", "main moves");
    git_run(work.path(), &["push", "-q", "origin", "main"]);
    git_run(work.path(), &["checkout", "-q", "feat"]);
    seed_commit(work.path(), "feat.txt", "f2\n", "f2 unpushed");

    // Teammate (or cleanup) deletes the remote branch; we prune.
    git_run(work.path(), &["push", "-q", "origin", "--delete", "feat"]);
    git_run(work.path(), &["fetch", "-q", "--prune", "origin"]);

    let st = list_status_with_base(&target, "main").unwrap();
    assert_eq!(st.branch.as_deref(), Some("feat"));
    assert_eq!(
        st.upstream.as_deref(),
        Some("origin/feat"),
        "upstream config survives the prune"
    );
    // 업스트림 ref 가 사라져 porcelain 의 `# branch.ab` 는 없지만, 폴백이
    // origin/main 에 없는 커밋 수(f1, f2)를 ahead 로 채운다 — "다음 할 일"이
    // 푸시를 제안할 근거가 살아난다.
    assert_eq!(
        st.ahead, 2,
        "fallback counts origin/main..HEAD after the prune"
    );
    assert_eq!(st.behind, 0);
    // behind_base is computed independently against origin/main and survives.
    assert_eq!(st.behind_base, 1, "origin/main is 1 ahead of this branch");
}
