//! 팀 전체 루프 시뮬레이션 — docs/WORKFLOW.md 의 "한 바퀴"를 crate 공개 API로
//! 끝까지 돌린다.
//!
//! 등장인물: 병합 관리자 민지(minji@t.com) + 팀원 준호/도윤/서연/하늘/지우.
//! 사람마다 bare origin 을 공유하는 별도 클론(자기 user.name/email)을 쓰고,
//! 앱 동작은 전부 `Target::Local` 을 받는 공개 API로만 수행한다. raw git 은
//! 셋업/검증에만 쓴다.
//!
//! BUG-n 테스트들은 수정된(올바른) 동작을 assert 하는 회귀 테스트다 —
//! 각 테스트의 주석이 원래 어떤 버그였는지 설명한다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use git_companion::git::fetch::fetch_target;
use git_companion::git::merge::{
    complete_merge, conflict_detail, delete_remote_branch, list_merged_remote_branches,
    list_pending_branches, resolve_conflict, start_merge, PendingBranch, Resolution,
};
use git_companion::git::ops::{list_status_with_base, StashAction};
use git_companion::git::{
    checkout_branch, commit, create_branch, list_status, push, stash, sync_to_base, Target,
};
use git_companion::gpconfig::{
    self, is_merge_target, member_from_account, read_config, read_config_effective, ProjectConfig,
};

// ── 공용 헬퍼 ───────────────────────────────────────────────────────────────

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .args(args)
        .output()
        .expect("git spawn");
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write_file(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, body).unwrap();
}

fn read_file(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap_or_default()
}

fn set_identity(dir: &Path, name: &str, email: &str) {
    git(dir, &["config", "user.name", name]);
    git(dir, &["config", "user.email", email]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

/// `git ls-tree -r --name-only <rev>` — bare 저장소에서도 동작한다.
fn tree_files(dir: &Path, rev: &str) -> Vec<String> {
    git(dir, &["ls-tree", "-r", "--name-only", rev])
        .lines()
        .map(|s| s.to_string())
        .collect()
}

fn assert_tree_has(dir: &Path, rev: &str, paths: &[&str], why: &str) {
    let files = tree_files(dir, rev);
    for p in paths {
        assert!(
            files.iter().any(|f| f == p),
            "{why}: {rev} 트리에 {p} 가 없다 (있는 것: {files:?})"
        );
    }
}

/// 한 사람 = bare origin 을 향한 클론 하나.
struct Person {
    dir: TempDir,
}

impl Person {
    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn target(&self) -> Target {
        Target::Local(PathBuf::from(self.dir.path()))
    }
}

/// 공유 bare origin.
struct Rig {
    bare: TempDir,
    url: String,
}

impl Rig {
    fn new() -> Rig {
        let bare = TempDir::new().unwrap();
        git(
            bare.path(),
            &["init", "--bare", "-q", "--initial-branch=main"],
        );
        let url = format!("file://{}", bare.path().display());
        Rig { bare, url }
    }

    /// 저장소를 처음 만든 사람 — init → 첫 커밋 → origin 등록 → push.
    fn seed_manager(&self, name: &str, email: &str, files: &[(&str, &str)]) -> Person {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        set_identity(dir.path(), name, email);
        for (rel, body) in files {
            write_file(dir.path(), rel, body);
        }
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "init: 프로젝트 시작"]);
        git(dir.path(), &["remote", "add", "origin", &self.url]);
        git(dir.path(), &["push", "-q", "-u", "origin", "main"]);
        git(dir.path(), &["fetch", "-q", "origin"]);
        Person { dir }
    }

    /// 팀원 합류 — `git clone` (origin/HEAD 도 생긴다).
    fn clone_person(&self, name: &str, email: &str) -> Person {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["clone", "-q", &self.url, "."]);
        set_identity(dir.path(), name, email);
        Person { dir }
    }
}

const TEAM: [(&str, &str, &str); 6] = [
    ("민지", "minji@t.com", "admin"),
    ("준호", "junho@t.com", "member"),
    ("도윤", "doyun@t.com", "member"),
    ("서연", "seoyeon@t.com", "member"),
    ("하늘", "haneul@t.com", "member"),
    ("지우", "jiwoo@t.com", "member"),
];

/// 6인 팀 `.gpconfig` — merge_managers 는 브랜치별, 관리자는 민지.
fn team_config(base: &str, targets: &[&str], manager_branch: &str) -> ProjectConfig {
    let mut cfg = ProjectConfig::default();
    cfg.default_base_branch = base.to_string();
    for (name, email, role) in TEAM {
        cfg.members.push(member_from_account("", name, email, role));
    }
    cfg.merge_managers
        .insert(manager_branch.to_string(), "minji@t.com".to_string());
    cfg.merge_targets = targets.iter().map(|s| s.to_string()).collect();
    cfg
}

fn pending_by_name<'a>(pending: &'a [PendingBranch]) -> HashMap<&'a str, &'a PendingBranch> {
    pending
        .iter()
        .map(|b| (b.short_name.as_str(), b))
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// 시나리오 1 + 5 — 교과서적인 한 주, 두 바퀴. 단계마다 nextAction 이 먹는
// 숫자(status.upstream/ahead/behind, behind_base, pending 수)가 진실인지 검증.
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn scenario1_canonical_week_two_rounds_nothing_lost() {
    let rig = Rig::new();
    let minji = rig.seed_manager(
        "민지",
        "minji@t.com",
        &[("README.md", "git companion\n"), ("shared.txt", "line1\n")],
    );
    let mt = minji.target();

    // ── 관리자: .gpconfig 작성 → 커밋 → push — 규칙이 저장소 안에 실려 배포 ──
    let cfg = team_config("main", &["main"], "main");
    gpconfig::save_config(&mt, &cfg).unwrap();
    let out = gpconfig::commit_config(&mt).unwrap();
    assert!(out.ok, "config 커밋 실패: {}", out.message);
    let p = push(&mt, Some("main"), None).unwrap();
    assert!(p.ok, "관리자 push 실패: {}", p.message);

    // ── 팀원 5명 합류 (config 커밋 이후 클론) ──
    let junho = rig.clone_person("준호", "junho@t.com");
    let doyun = rig.clone_person("도윤", "doyun@t.com");
    let seoyeon = rig.clone_person("서연", "seoyeon@t.com");
    let haneul = rig.clone_person("하늘", "haneul@t.com");
    let jiwoo = rig.clone_person("지우", "jiwoo@t.com");

    // ── 라운드 1: 준호는 2커밋 (업스트림 추적 검증 포함), 나머지는 1커밋 ──
    let jt = junho.target();
    create_branch(&jt, "feature/junho").unwrap();
    write_file(junho.path(), "j1.txt", "j1\n");
    assert!(commit(&jt, "feat: j1 추가", true).unwrap().ok);

    // [시나리오 5] 첫 push 전: 업스트림이 없어 ahead 는 0 그대로다.
    let st = list_status(&jt).unwrap();
    assert!(st.upstream.is_none(), "push 전에는 upstream 이 없어야 한다");
    assert_eq!(st.ahead, 0);

    // 첫 push — ops::push 는 `-u` 로 업스트림을 함께 건다 (git/ops.rs:196-199).
    assert!(push(&jt, Some("feature/junho"), None).unwrap().ok);
    let st = list_status(&jt).unwrap();
    assert_eq!(
        st.upstream.as_deref(),
        Some("origin/feature/junho"),
        "첫 push 후 업스트림이 설정되어야 한다 (-u)"
    );
    assert_eq!((st.ahead, st.behind), (0, 0));

    // 두 번째 커밋 → 업스트림이 살아 있으니 ahead 가 즉시 1 이 된다.
    write_file(junho.path(), "j2.txt", "j2\n");
    assert!(commit(&jt, "feat: j2 추가", true).unwrap().ok);
    let st = list_status(&jt).unwrap();
    assert_eq!(st.ahead, 1, "커밋 후 ahead 가 실시간으로 늘어야 한다");
    assert!(push(&jt, Some("feature/junho"), None).unwrap().ok);
    assert_eq!(list_status(&jt).unwrap().ahead, 0, "push 후 ahead=0");

    let others: [(&Person, &str, &str, &str); 4] = [
        (&doyun, "feature/doyun", "d.txt", "feat: d 추가"),
        (&seoyeon, "feature/seoyeon", "s.txt", "feat: s 추가"),
        (&haneul, "feature/haneul", "h.txt", "feat: h 추가"),
        (&jiwoo, "feature/jiwoo", "w.txt", "feat: w 추가"),
    ];
    for (p, branch, file, msg) in others {
        let t = p.target();
        create_branch(&t, branch).unwrap();
        write_file(p.path(), file, &format!("{file} 내용\n"));
        assert!(commit(&t, msg, true).unwrap().ok);
        assert!(push(&t, Some(branch), None).unwrap().ok);
    }

    // ── 관리자: 병합 탭 = fetch --prune 후 대기 목록 — 정확히 5개, ahead/작성자 ──
    fetch_target(&mt, "origin").unwrap();
    let pending = list_pending_branches(&mt, "origin", "main").unwrap();
    assert_eq!(pending.len(), 5, "대기 브랜치는 정확히 5개: {pending:?}");
    let by = pending_by_name(&pending);
    let expect: [(&str, u32, &str); 5] = [
        ("feature/junho", 2, "준호"),
        ("feature/doyun", 1, "도윤"),
        ("feature/seoyeon", 1, "서연"),
        ("feature/haneul", 1, "하늘"),
        ("feature/jiwoo", 1, "지우"),
    ];
    for (name, ahead, author) in expect {
        let b = by.get(name).unwrap_or_else(|| panic!("{name} 누락"));
        assert_eq!(b.ahead, ahead, "{name} ahead");
        assert_eq!(b.behind, 0, "{name} behind");
        assert_eq!(b.author, author, "{name} 작성자");
        assert!(!b.local && !b.merged_locally);
    }

    // ── 관리자: 5개 전부 병합 (겹치는 파일 없음 → 충돌 없음) → push ──
    for name in [
        "feature/junho",
        "feature/doyun",
        "feature/seoyeon",
        "feature/haneul",
        "feature/jiwoo",
    ] {
        let o = start_merge(&mt, &format!("origin/{name}"), "main", "origin", None).unwrap();
        assert!(o.ok && !o.conflicted, "{name} 병합 실패: {}", o.message);
    }
    assert!(push(&mt, Some("main"), None).unwrap().ok);

    // 데이터 손실 없음 — origin(main) 에 모두의 라운드1 파일이 있다.
    assert_tree_has(
        rig.bare.path(),
        "main",
        &[
            "README.md", "shared.txt", ".gpconfig", "j1.txt", "j2.txt", "d.txt", "s.txt",
            "h.txt", "w.txt",
        ],
        "라운드1 병합 후 origin/main",
    );
    // 병합 후 대기 목록은 빈다.
    fetch_target(&mt, "origin").unwrap();
    assert!(
        list_pending_branches(&mt, "origin", "main").unwrap().is_empty(),
        "병합·push 후 대기 목록은 비어야 한다"
    );

    // ── 팀원: behind_base 가 '정확히' 병합된 커밋 수 → sync → 0 ──
    // origin/main 신규 커밋 = 기능 커밋 6(준호2+나머지4) + 병합 커밋 5 = 11.
    // 각자 자기 커밋은 이미 갖고 있으므로 behind_base = 11 - 자기 커밋 수.
    let round1_files = [
        "j1.txt", "j2.txt", "d.txt", "s.txt", "h.txt", "w.txt", ".gpconfig",
    ];
    let members: [(&Person, u32); 5] = [
        (&junho, 9),   // 11 - 2
        (&doyun, 10),  // 11 - 1
        (&seoyeon, 10),
        (&haneul, 10),
        (&jiwoo, 10),
    ];
    for (p, expected_behind) in members {
        let t = p.target();
        fetch_target(&t, "origin").unwrap();
        let st = list_status_with_base(&t, "main").unwrap();
        assert_eq!(
            st.behind_base, expected_behind,
            "behind_base 는 병합으로 새로 생긴 커밋 수와 정확히 같아야 한다 ({})",
            p.path().display()
        );
        let r = sync_to_base(&t, "main", "origin").unwrap();
        assert!(!r.conflicted, "동기화 충돌이 나면 안 된다: {}", r.message);
        let st = list_status_with_base(&t, "main").unwrap();
        assert_eq!(st.behind_base, 0, "동기화 후 behind_base=0");
        for f in round1_files {
            assert!(
                p.path().join(f).exists(),
                "동기화 후 {} 작업 트리에 {f} 가 있어야 한다",
                p.path().display()
            );
        }
    }

    // ═══ 라운드 2 — 같은 브랜치 위에 계속. 준호·도윤이 shared.txt 를 놓고 충돌 ═══
    write_file(junho.path(), "shared.txt", "line1-junho\n");
    assert!(commit(&jt, "r2: 준호 shared 수정", true).unwrap().ok);
    assert!(push(&jt, Some("feature/junho"), None).unwrap().ok);

    let dt = doyun.target();
    write_file(doyun.path(), "shared.txt", "line1-doyun\n");
    assert!(commit(&dt, "r2: 도윤 shared 수정", true).unwrap().ok);
    assert!(push(&dt, Some("feature/doyun"), None).unwrap().ok);

    for (p, branch, file) in [
        (&seoyeon, "feature/seoyeon", "r2s.txt"),
        (&haneul, "feature/haneul", "r2h.txt"),
        (&jiwoo, "feature/jiwoo", "r2w.txt"),
    ] {
        let t = p.target();
        write_file(p.path(), file, "라운드2\n");
        assert!(commit(&t, &format!("r2: {file}"), true).unwrap().ok);
        assert!(push(&t, Some(branch), None).unwrap().ok);
    }

    // 관리자: 대기 5개, 각자 ahead=2 (라운드1 동기화 병합 커밋 + r2 커밋).
    fetch_target(&mt, "origin").unwrap();
    let pending = list_pending_branches(&mt, "origin", "main").unwrap();
    assert_eq!(pending.len(), 5, "라운드2 대기도 정확히 5개");
    for b in &pending {
        assert_eq!(b.ahead, 2, "{}: 동기화 병합 + r2 커밋 = 2", b.short_name);
        assert_eq!(b.behind, 0, "{}: 동기화 직후라 behind=0", b.short_name);
    }

    // 준호 먼저 병합(성공) → 도윤 병합은 충돌 → 해결 → complete_merge.
    let o = start_merge(&mt, "origin/feature/junho", "main", "origin", None).unwrap();
    assert!(o.ok);
    let o = start_merge(&mt, "origin/feature/doyun", "main", "origin", None).unwrap();
    assert!(o.conflicted, "shared.txt 충돌이어야 한다");
    assert_eq!(o.conflicted_files, vec!["shared.txt".to_string()]);
    let d = conflict_detail(&mt, "shared.txt").unwrap();
    assert!(d.ours.contains("line1-junho"), "ours=현재 main(준호 반영)");
    assert!(d.theirs.contains("line1-doyun"), "theirs=가져온 도윤 브랜치");
    let remaining = resolve_conflict(
        &mt,
        "shared.txt",
        &Resolution::Manual {
            content: "line1-merged\n".into(),
        },
    )
    .unwrap();
    assert!(remaining.is_empty());
    let done = complete_merge(&mt, Some("feature/doyun 브랜치 병합")).unwrap();
    assert!(done.ok, "{}", done.message);

    for name in ["feature/seoyeon", "feature/haneul", "feature/jiwoo"] {
        let o = start_merge(&mt, &format!("origin/{name}"), "main", "origin", None).unwrap();
        assert!(o.ok && !o.conflicted, "{name}: {}", o.message);
    }
    assert!(push(&mt, Some("main"), None).unwrap().ok);

    // origin/main: 라운드2 파일 + 수동 해결 내용, 잃은 것 없음.
    assert_tree_has(
        rig.bare.path(),
        "main",
        &["r2s.txt", "r2h.txt", "r2w.txt", "j1.txt", "d.txt"],
        "라운드2 병합 후 origin/main",
    );
    assert_eq!(
        git(rig.bare.path(), &["show", "main:shared.txt"]),
        "line1-merged\n",
        "관리자의 수동 해결 결과가 push 되어야 한다"
    );

    // 팀원: behind_base = 15 - 자기 것 2 = 13 (5명 모두), sync 후 0 + 파일 전수.
    let all_files = [
        "j1.txt", "j2.txt", "d.txt", "s.txt", "h.txt", "w.txt", "r2s.txt", "r2h.txt", "r2w.txt",
    ];
    for p in [&junho, &doyun, &seoyeon, &haneul, &jiwoo] {
        let t = p.target();
        fetch_target(&t, "origin").unwrap();
        let st = list_status_with_base(&t, "main").unwrap();
        assert_eq!(
            st.behind_base, 13,
            "라운드2 후 behind_base = 신규 15(동기화병합5+r2 5+병합커밋5) - 자기 2"
        );
        let r = sync_to_base(&t, "main", "origin").unwrap();
        assert!(!r.conflicted, "이미 병합된 브랜치의 동기화는 충돌 없음: {}", r.message);
        assert_eq!(list_status_with_base(&t, "main").unwrap().behind_base, 0);
        for f in all_files {
            assert!(p.path().join(f).exists(), "{f} 손실");
        }
        assert_eq!(read_file(p.path(), "shared.txt"), "line1-merged\n");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 시나리오 2 — commit_config 위생: 팀원이 미리 스테이징해 둔 다른 파일은
// "chore: update project config" 커밋에 쓸려 들어가지 않아야 한다.
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn scenario2_commit_config_commits_only_gpconfig_leaves_staged_work_alone() {
    let rig = Rig::new();
    let junho = rig.seed_manager("준호", "junho@t.com", &[("a.txt", "v1\n")]);
    let t = junho.target();

    // 팀원의 진행 중인 작업: staged 파일 하나 + unstaged 수정 하나.
    write_file(junho.path(), "staged-work.txt", "내 기능 작업 중\n");
    git(junho.path(), &["add", "staged-work.txt"]);
    write_file(junho.path(), "a.txt", "unstaged 수정\n");

    // 설정 탭 저장 → 커밋 (앱 경로 그대로).
    gpconfig::save_config(&t, &team_config("main", &["main"], "main")).unwrap();
    let out = gpconfig::commit_config(&t).unwrap();
    assert!(out.ok);

    let subject = git(junho.path(), &["log", "-1", "--format=%s"]);
    assert_eq!(subject.trim(), "chore: update project config (.gpconfig)");
    let committed = git(junho.path(), &["show", "--name-only", "--format=", "HEAD"]);
    assert!(committed.contains(".gpconfig"), ".gpconfig 는 커밋됐다");

    // 회귀 방지(BUG-1): 과거에는 commit_config 가 pathspec 없는 `git commit`
    // 으로 인덱스 전체를 커밋해, 팀원이 스테이징해 둔 무관한 작업 파일까지
    // "chore: update project config (.gpconfig)" 커밋에 쓸려 들어갔다.
    // 지금은 `git commit -m … -- .gpconfig` (pathspec 제한)로 .gpconfig 만
    // 커밋하고, 스테이징된 다른 파일은 인덱스에 그대로 남긴다.
    assert!(
        !committed.contains("staged-work.txt"),
        "스테이징된 다른 파일이 config 커밋에 포함되면 안 된다: {committed}"
    );
    let st = list_status(&t).unwrap();
    assert!(
        st.files
            .iter()
            .any(|f| f.path == "staged-work.txt" && f.staged),
        "staged-work.txt 는 커밋되지 않고 스테이징 상태로 남아야 한다"
    );

    // unstaged 수정은 커밋되지 않고 작업 트리에 남는다.
    assert_eq!(
        git(junho.path(), &["show", "HEAD:a.txt"]),
        "v1\n",
        "unstaged 수정은 커밋에 안 들어간다"
    );
    let st = list_status(&t).unwrap();
    assert!(
        st.files.iter().any(|f| f.path == "a.txt" && f.unstaged),
        "unstaged 수정은 작업 트리에 남아야 한다"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 시나리오 3 — .gpconfig 사본이 없는 브랜치에서도 팀 규칙이 보이는가.
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn scenario3_rules_visible_from_branch_without_config_copy() {
    let rig = Rig::new();
    let minji = rig.seed_manager("민지", "minji@t.com", &[("README.md", "x\n")]);

    // 준호는 .gpconfig 가 생기기 **전에** 클론하고 브랜치를 갈랐다.
    let junho = rig.clone_person("준호", "junho@t.com");
    let jt = junho.target();
    create_branch(&jt, "feature/early").unwrap();
    write_file(junho.path(), "e.txt", "early\n");
    assert!(commit(&jt, "feat: early", true).unwrap().ok);

    // 그 뒤에야 민지가 규칙을 main 에 커밋·push 한다.
    let mt = minji.target();
    gpconfig::save_config(&mt, &team_config("main", &["main"], "main")).unwrap();
    assert!(gpconfig::commit_config(&mt).unwrap().ok);
    assert!(push(&mt, Some("main"), None).unwrap().ok);

    // 준호: fetch 만 하면 (checkout/merge 없이) 규칙이 보여야 한다.
    fetch_target(&jt, "origin").unwrap();
    let (_, plain) = read_config(&jt).unwrap();
    assert!(!plain, "작업 트리에는 .gpconfig 사본이 없다");
    let (cfg, exists) = read_config_effective(&jt, "main", "origin").unwrap();
    assert!(exists, "origin/main 에 커밋된 팀 규칙을 찾아야 한다");
    assert_eq!(
        cfg.merge_managers.get("main").map(String::as_str),
        Some("minji@t.com"),
        "팀원도 병합 관리자가 누구인지 알아야 한다"
    );
    assert!(is_merge_target(&cfg, exists, "main", "main"));
    assert!(!is_merge_target(&cfg, exists, "main", "feature/early"));

    // 로컬 main ref 가 아예 없는 클론 (origin/main 만 있음) — 여전히 찾는다.
    git(junho.path(), &["branch", "-q", "-D", "main"]);
    let (cfg, exists) = read_config_effective(&jt, "main", "origin").unwrap();
    assert!(exists, "로컬 main 없이 origin/main 만으로도 찾아야 한다");
    assert_eq!(
        cfg.merge_managers.get("main").map(String::as_str),
        Some("minji@t.com")
    );
}

/// .gpconfig 가 **origin/develop 에만** 있는 팀 (커스텀 병합 브랜치).
#[test]
fn scenario3_rules_only_on_origin_develop() {
    let rig = Rig::new();
    let minji = rig.seed_manager("민지", "minji@t.com", &[("README.md", "x\n")]);
    let mt = minji.target();

    // 규칙은 develop 에만 커밋·push. main 에는 .gpconfig 가 없다.
    create_branch(&mt, "develop").unwrap();
    gpconfig::save_config(&mt, &team_config("develop", &["develop"], "develop")).unwrap();
    assert!(gpconfig::commit_config(&mt).unwrap().ok);
    git(minji.path(), &["push", "-q", "origin", "develop"]);

    // 준호는 main 기준으로 클론해 feature 브랜치에 있다 — 작업 트리에 규칙 없음.
    let junho = rig.clone_person("준호", "junho@t.com");
    let jt = junho.target();
    create_branch(&jt, "feature/x").unwrap();
    let (_, plain) = read_config(&jt).unwrap();
    assert!(!plain);

    // base 를 develop 으로 물으면 찾는다.
    let (cfg, exists) = read_config_effective(&jt, "develop", "origin").unwrap();
    assert!(exists, "origin/develop 의 규칙을 찾아야 한다");
    assert!(is_merge_target(&cfg, exists, "main", "develop"));
    assert!(!is_merge_target(&cfg, exists, "main", "main"));

    // 회귀 방지(BUG-3): 과거에는 read_config_effective 의 후보가 등록 base
    // (origin/main·main)뿐이라, 규칙이 origin/develop 에만 있으면
    // exists=false → is_merge_target(develop)=false → develop push 가
    // branch_push 로 잘못 분류됐다. 지금은 원격 HEAD(origin/HEAD)가 가리키는
    // 기본 브랜치도 fallback 후보에 들어간다.
    //
    // 준호의 clone 은 clone 시점의 원격 HEAD(main)를 갖고 있다 — origin/main
    // 에는 규칙이 없으므로 아직은 못 찾는다.
    let (_, exists2) = read_config_effective(&jt, "main", "origin").unwrap();
    assert!(!exists2, "원격 HEAD 가 main 인 동안은 (규칙이 develop 에만 있어) 못 찾는다");

    // 호스트의 기본 브랜치가 develop 으로 바뀌면 (bare 의 HEAD 변경 후
    // fetch + remote set-head 로 origin/HEAD 갱신) —
    git(rig.bare.path(), &["symbolic-ref", "HEAD", "refs/heads/develop"]);
    fetch_target(&jt, "origin").unwrap();
    git(junho.path(), &["remote", "set-head", "origin", "--auto"]);

    // 등록 base 가 main 이어도 origin/HEAD(=develop) 의 .gpconfig 를 찾는다.
    let (cfg2, exists2) = read_config_effective(&jt, "main", "origin").unwrap();
    assert!(exists2, "원격 HEAD(develop) 에 커밋된 규칙을 찾아야 한다");
    assert!(
        is_merge_target(&cfg2, exists2, "main", "develop"),
        "develop push 가 병합 브랜치 push 로 올바르게 분류된다"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 시나리오 4 — 커스텀 병합 브랜치 develop (+ release/1.0) 전체 루프,
// 알림 분류, 삭제 가드.
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn scenario4_custom_merge_targets_develop_and_release() {
    let rig = Rig::new();
    let minji = rig.seed_manager("민지", "minji@t.com", &[("README.md", "x\n")]);
    let mt = minji.target();

    // develop 생성 + 규칙(develop, release/1.0) 커밋 → develop/release 로 push.
    create_branch(&mt, "develop").unwrap();
    gpconfig::save_config(
        &mt,
        &team_config("develop", &["develop", "release/1.0"], "develop"),
    )
    .unwrap();
    assert!(gpconfig::commit_config(&mt).unwrap().ok);
    git(minji.path(), &["push", "-q", "origin", "develop"]);
    git(
        minji.path(),
        &["push", "-q", "origin", "develop:refs/heads/release/1.0"],
    );
    git(minji.path(), &["fetch", "-q", "origin"]);

    // 팀원 2명: develop 에서 feature 를 갈라 커밋·push.
    let junho = rig.clone_person("준호", "junho@t.com");
    let doyun = rig.clone_person("도윤", "doyun@t.com");
    for (p, branch, file) in [
        (&junho, "feature/j4", "j4.txt"),
        (&doyun, "feature/d4", "d4.txt"),
    ] {
        let t = p.target();
        checkout_branch(&t, "develop").unwrap();
        create_branch(&t, branch).unwrap();
        write_file(p.path(), file, "작업\n");
        assert!(commit(&t, &format!("feat: {file}"), true).unwrap().ok);
        assert!(push(&t, Some(branch), None).unwrap().ok);
    }

    // 알림 분류 — 팀원 클론에서 실제로 읽히는 규칙으로 판정한다.
    let (cfg, exists) = read_config_effective(&junho.target(), "develop", "origin").unwrap();
    assert!(exists);
    assert!(is_merge_target(&cfg, exists, "main", "develop"), "develop push → 동기화 알림");
    assert!(
        is_merge_target(&cfg, exists, "main", "release/1.0"),
        "release/1.0 push → 동기화 알림"
    );
    assert!(!is_merge_target(&cfg, exists, "main", "feature/j4"), "feature push 는 관리자에게만");
    assert!(
        !is_merge_target(&cfg, exists, "main", "main"),
        "main 은 대상 목록에 없다 — 하드코딩 금지"
    );

    // 대기 목록(base=develop): feature 2개만. base 자신·main(조상)·
    // release/1.0(develop 과 동일 커밋=조상)은 제외.
    fetch_target(&mt, "origin").unwrap();
    let pending = list_pending_branches(&mt, "origin", "develop").unwrap();
    let names: Vec<&str> = pending.iter().map(|b| b.short_name.as_str()).collect();
    assert!(names.contains(&"feature/j4") && names.contains(&"feature/d4"), "{names:?}");
    assert!(!names.contains(&"develop") && !names.contains(&"main"));
    assert!(!names.contains(&"release/1.0"), "조상인 병합 대상은 대기 목록에 없다");

    // ── 삭제 가드 ──
    // merge.rs 계층: base(develop) 자신은 거부한다.
    let err = delete_remote_branch(&mt, "origin", "develop", "develop").unwrap_err();
    assert!(err.to_string().contains("삭제할 수 없습니다"), "{err}");

    // 커맨드 계층(commands/git.rs:235-251 merge_target_branches)이 추가하는
    // 보호 목록을 재현: base + .gpconfig 의 merge_targets + default_base_branch.
    // delete_remote_branch 커맨드는 이 목록에 든 브랜치를 무조건 거부한다.
    let mut protected = vec!["develop".to_string()];
    for t in &cfg.merge_targets {
        if !protected.contains(t) {
            protected.push(t.clone());
        }
    }
    if !cfg.default_base_branch.is_empty() && !protected.contains(&cfg.default_base_branch) {
        protected.push(cfg.default_base_branch.clone());
    }
    assert!(protected.contains(&"develop".to_string()));
    assert!(
        protected.contains(&"release/1.0".to_string()),
        "커맨드 계층에서는 release/1.0 도 삭제 거부 대상이다"
    );

    // merge 계층의 "병합 끝난 브랜치" 후보에는 release/1.0 이 그대로 나온다
    // (조상이므로) — 커맨드 계층(list_merged_remote_branches)이 걸러 준다.
    let merged = list_merged_remote_branches(&mt, "origin", "develop").unwrap();
    assert!(
        merged.iter().any(|b| b.short_name == "release/1.0"),
        "merge.rs 계층 후보 목록에는 병합 대상 브랜치가 노출된다"
    );

    // 회귀 방지(BUG-2): 과거에는 merge 계층 delete_remote_branch 의 가드가
    // `branch == base` 하나뿐이라, release/1.0 처럼 base 가 아닌 병합 대상
    // 브랜치가 origin/develop 의 조상인 동안 merge 계층을 직접 부르면 원격에서
    // 지워졌다 (보호는 커맨드 계층에만 있었다). 지금은 delete_remote_branch 가
    // .gpconfig 의 merge_targets/default_base_branch 도 스스로 거부한다.
    let err = delete_remote_branch(&mt, "origin", "develop", "release/1.0")
        .expect_err("병합 대상 브랜치 삭제는 merge 계층에서도 거부돼야 한다");
    assert!(
        err.to_string()
            .contains("병합 대상 브랜치라 삭제할 수 없습니다"),
        "{err}"
    );
    let ls = git(
        rig.bare.path(),
        &["ls-remote", "--heads", ".", "release/1.0"],
    );
    assert!(!ls.trim().is_empty(), "release/1.0 은 원격에 그대로 남아야 한다: {ls}");

    // ── develop 기준 병합 루프 + 팀원 동기화 ──
    for name in ["feature/j4", "feature/d4"] {
        let o = start_merge(&mt, &format!("origin/{name}"), "develop", "origin", None).unwrap();
        assert!(o.ok && !o.conflicted, "{name}: {}", o.message);
    }
    assert!(push(&mt, Some("develop"), None).unwrap().ok);
    assert_tree_has(
        rig.bare.path(),
        "develop",
        &["j4.txt", "d4.txt", ".gpconfig"],
        "develop 병합 후",
    );

    for (p, own) in [(&junho, 1u32), (&doyun, 1u32)] {
        let t = p.target();
        fetch_target(&t, "origin").unwrap();
        let st = list_status_with_base(&t, "develop").unwrap();
        // 신규 = 기능 2 + 병합 커밋 2 = 4, 자기 것 제외.
        assert_eq!(st.behind_base, 4 - own, "develop 기준 behind_base");
        let r = sync_to_base(&t, "develop", "origin").unwrap();
        assert!(!r.conflicted, "{}", r.message);
        assert_eq!(list_status_with_base(&t, "develop").unwrap().behind_base, 0);
        assert!(p.path().join("j4.txt").exists() && p.path().join("d4.txt").exists());
    }

    // ── release/1.0 이 갈라지면 (핫픽스) 대기 목록에 등장한다 ──
    git(
        minji.path(),
        &["checkout", "-q", "-b", "release/1.0", "origin/release/1.0"],
    );
    write_file(minji.path(), "hotfix.txt", "핫픽스\n");
    git(minji.path(), &["add", "-A"]);
    git(minji.path(), &["commit", "-q", "-m", "hotfix: 릴리스 수정"]);
    git(minji.path(), &["push", "-q", "origin", "release/1.0"]);
    fetch_target(&mt, "origin").unwrap();
    let pending = list_pending_branches(&mt, "origin", "develop").unwrap();
    // 회귀 방지(BUG-5): merge 계층 list_pending_branches 는 여전히 base 와
    // origin/HEAD 만 제외한다 — 갈라진 release/1.0 은 merge 계층 후보에 그대로
    // 나온다(의도된 계층 분리). 필터는 커맨드 계층
    // (commands/git.rs list_pending_branches)이 merge_target_branches 로
    // 수행한다. 커맨드 fn 은 repo_id(등록 저장소)와 Tauri 상태가 필요해 여기서
    // 직접 부를 수 없으므로, 커맨드 계층이 하는 필터를 그대로 재현해 검증한다.
    assert!(
        pending.iter().any(|b| b.short_name == "release/1.0"),
        "merge 계층 후보에는 갈라진 병합 대상 브랜치가 나온다: {pending:?}"
    );
    let filtered: Vec<&PendingBranch> = pending
        .iter()
        .filter(|b| {
            b.short_name != "develop"
                && !cfg.merge_targets.contains(&b.short_name)
                && b.short_name != cfg.default_base_branch
        })
        .collect();
    assert!(
        filtered.iter().all(|b| b.short_name != "release/1.0"),
        "커맨드 계층 필터를 거치면 release/1.0 은 대기 카드에서 제외된다"
    );
    assert!(
        filtered.is_empty(),
        "핫픽스 뒤 남은 대기 카드는 없어야 한다: {filtered:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 시나리오 5(보강) — 한 번도 push 안 한 브랜치: base 에 없는 커밋 수가
// ahead 로 잡혀 "커밋 N개 푸시" 신호가 만들어진다.
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn scenario5_never_pushed_branch_reports_push_signal() {
    let rig = Rig::new();
    let _minji = rig.seed_manager("민지", "minji@t.com", &[("README.md", "x\n")]);
    let junho = rig.clone_person("준호", "junho@t.com");
    let jt = junho.target();

    create_branch(&jt, "feature/silent").unwrap();
    write_file(junho.path(), "n.txt", "새 작업\n");
    assert!(commit(&jt, "feat: n 추가", true).unwrap().ok);
    write_file(junho.path(), "n2.txt", "새 작업 2\n");
    assert!(commit(&jt, "feat: n2 추가", true).unwrap().ok);

    // 실제로는 origin/main 에 없는 커밋이 2개 있다 — push 해야 관리자가 본다.
    let unpushed = git(junho.path(), &["rev-list", "--count", "origin/main..HEAD"]);
    assert_eq!(unpushed.trim(), "2");

    // 회귀 방지(BUG-4): 과거에는 업스트림이 없으면 porcelain `# branch.ab` 가
    // 아예 없어 ahead=0 → computeNextAction 입력이 전부 0 → "할 일 없음" 으로
    // 보였다(커밋은 로컬에만 있는데 push 를 권하지 않았다). 지금은
    // list_status_with_base 가 업스트림 부재/소실 시 origin/<base> 에 없는
    // 커밋 수를 ahead 로 채워 "커밋 N개 푸시" 신호를 만든다.
    let st = list_status_with_base(&jt, "main").unwrap();
    assert_eq!(st.branch.as_deref(), Some("feature/silent"));
    assert!(st.upstream.is_none(), "업스트림 없음");
    assert_eq!(st.ahead, 2, "push 할 커밋 2개가 ahead 로 잡혀야 한다");
    assert_eq!(st.behind, 0);
    assert_eq!(st.behind_base, 0);
    assert!(st.files.is_empty(), "작업 트리는 깨끗");
}

// ═══════════════════════════════════════════════════════════════════════════
// 시나리오 6 — 관리자도 개발자다: 자기 브랜치의 미커밋 변경 → 병합 거부 →
// stash → 병합 → push → stash pop, 아무것도 잃지 않는다.
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn scenario6_manager_dirty_tree_stash_merge_pop() {
    let rig = Rig::new();
    let minji = rig.seed_manager("민지", "minji@t.com", &[("README.md", "v1\n")]);
    let mt = minji.target();

    // 민지 자신의 feature 작업 — 커밋 안 한 수정이 있는 상태.
    create_branch(&mt, "feature/minji").unwrap();
    write_file(minji.path(), "README.md", "민지의 미커밋 수정\n");

    // 그때 준호의 push 가 도착한다.
    let junho = rig.clone_person("준호", "junho@t.com");
    let jt = junho.target();
    create_branch(&jt, "feature/junho").unwrap();
    write_file(junho.path(), "j.txt", "준호 작업\n");
    assert!(commit(&jt, "feat: j 추가", true).unwrap().ok);
    assert!(push(&jt, Some("feature/junho"), None).unwrap().ok);

    // 더러운 트리로는 병합이 거부된다 (친절한 안내 메시지).
    fetch_target(&mt, "origin").unwrap();
    let err = start_merge(&mt, "origin/feature/junho", "main", "origin", None).unwrap_err();
    assert!(
        err.to_string().contains("커밋되지 않은 변경"),
        "dirty-tree 거부 메시지: {err}"
    );

    // stash → 병합 → push.
    stash(
        &mt,
        StashAction::Save {
            message: Some("병합 전 임시 보관".into()),
        },
    )
    .unwrap();
    let o = start_merge(&mt, "origin/feature/junho", "main", "origin", None).unwrap();
    assert!(o.ok && !o.conflicted, "{}", o.message);
    assert!(push(&mt, Some("main"), None).unwrap().ok);
    assert_tree_has(rig.bare.path(), "main", &["j.txt"], "준호 작업 병합됨");

    // 자기 브랜치로 돌아가 stash pop — 미커밋 수정이 그대로 살아 있다.
    checkout_branch(&mt, "feature/minji").unwrap();
    stash(&mt, StashAction::Pop).unwrap();
    assert_eq!(
        read_file(minji.path(), "README.md"),
        "민지의 미커밋 수정\n",
        "관리자의 진행 중 작업을 잃으면 안 된다"
    );
    let st = list_status(&mt).unwrap();
    assert!(
        st.files.iter().any(|f| f.path == "README.md" && f.unstaged),
        "수정이 다시 작업 트리에 있다"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 시나리오 7 — push 후 원격 브랜치 삭제·개명: prune 전에는 유령이 남고,
// 병합 탭이 실제로 수행하는 fetch --prune(fetch_target) 후에는 현실을 반영.
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn scenario7_renamed_remote_branch_stale_until_prune() {
    let rig = Rig::new();
    let minji = rig.seed_manager("민지", "minji@t.com", &[("README.md", "x\n")]);
    let mt = minji.target();

    let junho = rig.clone_person("준호", "junho@t.com");
    let jt = junho.target();
    create_branch(&jt, "feature/old-name").unwrap();
    write_file(junho.path(), "j.txt", "작업\n");
    assert!(commit(&jt, "feat: j", true).unwrap().ok);
    assert!(push(&jt, Some("feature/old-name"), None).unwrap().ok);

    // 관리자가 한 번 보고 (fetch) — old-name 이 대기 목록에 있다.
    fetch_target(&mt, "origin").unwrap();
    let names: Vec<String> = list_pending_branches(&mt, "origin", "main")
        .unwrap()
        .into_iter()
        .map(|b| b.short_name)
        .collect();
    assert_eq!(names, vec!["feature/old-name".to_string()]);

    // 준호가 원격 브랜치를 지우고 새 이름으로 다시 push 한다.
    git(junho.path(), &["push", "-q", "origin", "--delete", "feature/old-name"]);
    git(junho.path(), &["branch", "-q", "-m", "feature/new-name"]);
    assert!(push(&jt, Some("feature/new-name"), None).unwrap().ok);

    // fetch 전: 관리자의 원격 트래킹 ref 는 그대로라 유령(old-name)이 남는다.
    let stale: Vec<String> = list_pending_branches(&mt, "origin", "main")
        .unwrap()
        .into_iter()
        .map(|b| b.short_name)
        .collect();
    assert_eq!(
        stale,
        vec!["feature/old-name".to_string()],
        "fetch 전에는 지워진 이름이 그대로 보인다 (트래킹 ref 기준)"
    );

    // 병합 탭이 목록을 채우기 전에 항상 수행하는 fetch --prune 을 그대로 실행.
    fetch_target(&mt, "origin").unwrap();
    let fresh: Vec<String> = list_pending_branches(&mt, "origin", "main")
        .unwrap()
        .into_iter()
        .map(|b| b.short_name)
        .collect();
    assert_eq!(
        fresh,
        vec!["feature/new-name".to_string()],
        "prune 후에는 새 이름만 남는다 — 유령 없음"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 시나리오 8 — 6개 저장소가 만드는 하나의 대기 목록: HEAD 포인터·base·
// 이미 병합된 브랜치 제외, 동일 tip 의 로컬+원격 중복 제거, 한글 작성자.
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn scenario8_six_repos_one_pending_list_exclusions_and_dedup() {
    let rig = Rig::new();
    // 씨앗은 별도 데스크톱에서 만들고, 민지는 clone 으로 합류 → origin/HEAD 존재.
    let _seed = rig.seed_manager("민지", "minji@t.com", &[("README.md", "x\n")]);
    let minji = rig.clone_person("민지", "minji@t.com");
    let mt = minji.target();
    git(minji.path(), &["remote", "set-head", "origin", "main"]);
    // origin/HEAD 가 정말 있어서 "제외"가 의미 있는지 먼저 확인한다.
    // (주의: %(refname:short)는 refs/remotes/origin/HEAD 를 "origin" 으로 줄인다.)
    let refs = git(
        minji.path(),
        &["for-each-ref", "refs/remotes/origin", "--format=%(refname)"],
    );
    assert!(
        refs.contains("refs/remotes/origin/HEAD"),
        "전제: origin/HEAD 존재 ({refs})"
    );

    // 지우의 옛 브랜치: push → 병합 → main push 완료 = 완전히 병합된 잔재.
    let jiwoo = rig.clone_person("지우", "jiwoo@t.com");
    {
        let t = jiwoo.target();
        create_branch(&t, "feature/stale").unwrap();
        write_file(jiwoo.path(), "stale.txt", "옛 작업\n");
        assert!(commit(&t, "feat: stale", true).unwrap().ok);
        assert!(push(&t, Some("feature/stale"), None).unwrap().ok);
    }
    fetch_target(&mt, "origin").unwrap();
    let o = start_merge(&mt, "origin/feature/stale", "main", "origin", None).unwrap();
    assert!(o.ok);
    assert!(push(&mt, Some("main"), None).unwrap().ok);

    // 5명의 살아 있는 브랜치.
    let people: [(&str, &str, &str, &str); 5] = [
        ("준호", "junho@t.com", "feature/junho", "j.txt"),
        ("도윤", "doyun@t.com", "feature/doyun", "d.txt"),
        ("서연", "seoyeon@t.com", "feature/seoyeon", "s.txt"),
        ("하늘", "haneul@t.com", "feature/haneul", "h.txt"),
        ("지우", "jiwoo@t.com", "feature/jiwoo", "w.txt"),
    ];
    for (name, email, branch, file) in people {
        let p = rig.clone_person(name, email);
        let t = p.target();
        create_branch(&t, branch).unwrap();
        write_file(p.path(), file, "작업\n");
        assert!(commit(&t, &format!("feat: {file}"), true).unwrap().ok);
        assert!(push(&t, Some(branch), None).unwrap().ok);
    }

    // 관리자: fetch 후 준호 브랜치를 로컬로 체크아웃 (동일 tip 의 로컬 사본).
    fetch_target(&mt, "origin").unwrap();
    checkout_branch(&mt, "origin/feature/junho").unwrap();

    let pending = list_pending_branches(&mt, "origin", "main").unwrap();
    let by = pending_by_name(&pending);
    assert_eq!(
        pending.len(),
        5,
        "정확히 5개 — HEAD 포인터/base/병합 완료/중복 제외: {:?}",
        pending.iter().map(|b| &b.short_name).collect::<Vec<_>>()
    );
    for (_, _, branch, _) in people {
        assert!(by.contains_key(branch), "{branch} 누락");
    }
    assert!(!by.contains_key("main"), "base 자신은 제외");
    assert!(!by.contains_key("HEAD") && !by.contains_key("origin/HEAD"), "HEAD 포인터 제외");
    assert!(!by.contains_key("feature/stale"), "이미 병합된 브랜치 제외");
    // 로컬 feature/junho 는 원격 tip 과 동일 sha → 원격 항목 하나로 dedup.
    let j = by["feature/junho"];
    assert!(!j.local, "동일 tip 로컬+원격은 원격 형태 하나로 합쳐진다");
    // 한글 작성자 이름이 그대로 실린다.
    let authors: HashMap<&str, &str> = [
        ("feature/junho", "준호"),
        ("feature/doyun", "도윤"),
        ("feature/seoyeon", "서연"),
        ("feature/haneul", "하늘"),
        ("feature/jiwoo", "지우"),
    ]
    .into_iter()
    .collect();
    for (branch, author) in authors {
        assert_eq!(by[branch].author, author, "{branch} 작성자");
    }

    // ── origin/HEAD 가 base 가 아닌 브랜치를 가리켜도 유령이 없어야 한다 ──
    // 회귀 방지(BUG-6): 과거에는 %(refname:short)가 refs/remotes/origin/HEAD
    // 를 "origin" 으로 줄여 이름 필터("origin/HEAD"/"HEAD")가 절대 매치되지
    // 않았고, 원격 HEAD 가 base 가 아닌 브랜치를 가리키는 순간 같은 커밋의
    // feature/junho 와 중복인 "origin" 유령 카드가 나타났다. 지금은
    // %(symref) 로 심볼릭 ref 자체를 걸러 유령이 생기지 않는다.
    git(
        minji.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/feature/junho",
        ],
    );
    let pending = list_pending_branches(&mt, "origin", "main").unwrap();
    let names: Vec<&str> = pending.iter().map(|b| b.short_name.as_str()).collect();
    assert_eq!(pending.len(), 5, "유령 없이 그대로 5개여야 한다: {names:?}");
    assert!(
        !names.contains(&"origin"),
        "origin/HEAD 가 'origin' 카드로 노출되면 안 된다: {names:?}"
    );
}
