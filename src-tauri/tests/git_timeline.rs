//! merge_timeline 통합 테스트 — 실제 git 저장소(bare origin + clone) 위에서
//! 병합 흐름이 올바르게 복원되는지 확인한다.
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use git_companion::git::timeline::merge_timeline;
use git_companion::git::Target;

fn git_at(dir: &Path, args: &[&str], date: Option<&str>) {
    let mut c = std::process::Command::new("git");
    c.args(args)
        .current_dir(dir)
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .env("GIT_TERMINAL_PROMPT", "0");
    if let Some(d) = date {
        c.env("GIT_AUTHOR_DATE", d).env("GIT_COMMITTER_DATE", d);
    }
    let out = c.output().unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write(repo: &Path, rel: &str, body: &str) {
    let full = repo.join(rel);
    if let Some(p) = full.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(full, body).unwrap();
}

/// `days_ago`일 전 정오의 RFC3339 시각 — 커밋 날짜 고정용.
fn days_ago(n: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(n))
        .format("%Y-%m-%dT12:00:00+00:00")
        .to_string()
}

/// bare origin + clone. main 초기 커밋(6일 전)까지 만들어 둔다.
fn fixture() -> (TempDir, PathBuf) {
    let td = TempDir::new().unwrap();
    let origin = td.path().join("origin.git");
    let repo = td.path().join("work");
    git_at(td.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()], None);
    git_at(td.path(), &["clone", "-q", origin.to_str().unwrap(), repo.to_str().unwrap()], None);
    git_at(&repo, &["config", "user.email", "minji@example.com"], None);
    git_at(&repo, &["config", "user.name", "김민지"], None);
    git_at(&repo, &["config", "commit.gpgsign", "false"], None);
    git_at(&repo, &["checkout", "-q", "-B", "main"], None);
    write(&repo, "README.md", "init\n");
    git_at(&repo, &["add", "-A"], None);
    git_at(&repo, &["commit", "-qm", "init"], Some(&days_ago(6)));
    git_at(&repo, &["push", "-q", "-u", "origin", "main"], None);
    (td, repo)
}

#[test]
fn timeline_reconstructs_merge_direct_and_open_branches() {
    let (_td, repo) = fixture();

    // feature/login: 커밋 2개(5일/4일 전) → 3일 전 main 에 병합(팀 컨벤션 문구).
    git_at(&repo, &["checkout", "-qb", "feature/login"], None);
    write(&repo, "src/auth.ts", "a\n");
    git_at(&repo, &["add", "-A"], None);
    git_at(&repo, &["commit", "-qm", "feat: 로그인 1"], Some(&days_ago(5)));
    write(&repo, "src/token.ts", "t\n");
    git_at(&repo, &["add", "-A"], None);
    git_at(&repo, &["commit", "-qm", "feat: 로그인 2"], Some(&days_ago(4)));
    git_at(&repo, &["checkout", "-q", "main"], None);
    git_at(
        &repo,
        &["merge", "--no-ff", "-m", "feature/login 브렌치 병합", "feature/login"],
        Some(&days_ago(3)),
    );

    // main 직접 커밋 (2일 전).
    write(&repo, "README.md", "init\nmore\n");
    git_at(&repo, &["add", "-A"], None);
    git_at(&repo, &["commit", "-qm", "docs: readme"], Some(&days_ago(2)));
    git_at(&repo, &["push", "-q", "origin", "main"], None);

    // 아직 병합 안 된 브랜치 (1일 전 push).
    git_at(&repo, &["checkout", "-qb", "feature/wip"], None);
    write(&repo, "src/wip.ts", "w\n");
    git_at(&repo, &["add", "-A"], None);
    git_at(&repo, &["commit", "-qm", "wip: 작업 중"], Some(&days_ago(1)));
    git_at(&repo, &["push", "-q", "-u", "origin", "feature/wip"], None);
    git_at(&repo, &["checkout", "-q", "main"], None);

    let target = Target::Local(repo.clone());
    let tl = merge_timeline(&target, "origin", "main", 7).unwrap();

    assert_eq!(tl.base, "main");
    assert_eq!(tl.merges.len(), 1, "merges: {:?}", tl.merges);
    let m = &tl.merges[0];
    assert_eq!(m.branch.as_deref(), Some("feature/login"));
    assert_eq!(m.commits.len(), 2);
    assert_eq!(m.files, vec!["src/auth.ts", "src/token.ts"]);
    assert!(m.first_commit_date.is_some());
    // 레인 시작(첫 커밋 작성일)이 병합일보다 앞선다.
    assert!(m.first_commit_date.as_deref().unwrap() < m.date.as_str());

    // 직접 커밋: init(6일 전)과 docs(2일 전) 모두 7일 창 안.
    let direct_subjects: Vec<&str> = tl.direct.iter().map(|c| c.subject.as_str()).collect();
    assert!(direct_subjects.contains(&"docs: readme"), "{direct_subjects:?}");

    assert_eq!(tl.open.len(), 1, "open: {:?}", tl.open);
    assert_eq!(tl.open[0].name, "feature/wip");
    assert_eq!(tl.open[0].commits.len(), 1);
    assert_eq!(tl.open[0].files, vec!["src/wip.ts"]);
}

#[test]
fn merge_older_than_window_is_excluded() {
    let (_td, repo) = fixture();
    git_at(&repo, &["checkout", "-qb", "feature/old"], None);
    write(&repo, "old.ts", "o\n");
    git_at(&repo, &["add", "-A"], None);
    git_at(&repo, &["commit", "-qm", "feat: old"], Some(&days_ago(12)));
    git_at(&repo, &["checkout", "-q", "main"], None);
    git_at(
        &repo,
        &["merge", "--no-ff", "-m", "feature/old 브렌치 병합", "feature/old"],
        Some(&days_ago(10)),
    );
    git_at(&repo, &["push", "-q", "origin", "main"], None);

    let target = Target::Local(repo.clone());
    let tl = merge_timeline(&target, "origin", "main", 7).unwrap();
    assert!(tl.merges.is_empty(), "10일 전 병합은 7일 창 밖: {:?}", tl.merges);
    // 병합된 브랜치가 "열린 브랜치"로 둔갑하지도 않는다.
    assert!(tl.open.iter().all(|b| b.name != "feature/old"), "{:?}", tl.open);
}

#[test]
fn unborn_or_missing_base_yields_empty_timeline() {
    // 커밋이 하나도 없는 저장소 — 빈 타임라인이어야지 오류면 안 된다.
    let td = TempDir::new().unwrap();
    git_at(td.path(), &["init", "-q", "-b", "main"], None);
    let target = Target::Local(td.path().to_path_buf());
    let tl = merge_timeline(&target, "origin", "main", 7).unwrap();
    assert!(tl.merges.is_empty() && tl.direct.is_empty() && tl.open.is_empty());

    // 존재하지 않는 base 이름도 마찬가지.
    let (_td2, repo) = fixture();
    let target2 = Target::Local(repo);
    let tl2 = merge_timeline(&target2, "origin", "no-such-branch", 7).unwrap();
    assert!(tl2.merges.is_empty() && tl2.open.is_empty());
}
