//! 팀 시뮬레이션 — 병합 관리자(민지)와 공유 origin 의 "절대 휘발 금지" 검증.
//!
//! 두 불변식을 모든 시나리오의 모든 연산 뒤에 기계적으로 검사한다:
//!
//! (A) origin 은 커밋을 잃지 않는다 — 누군가 push 해서 origin ref 에서 도달
//!     가능해진 SHA 는 영원히 origin ref 에서 도달 가능해야 한다. 유일한
//!     예외는 delete_remote_branch 이고, 그때도 지워진 브랜치의 모든 SHA 가
//!     origin/<base> 에서 도달 가능할 때만 허용된다(가드의 존재 이유).
//!     → `OriginLedger`: 모든 push 직후 origin 의 각 ref 에서 도달 가능한
//!       SHA 전부를 장부에 적고, 이후 매 연산 뒤 `assert_origin_intact`.
//!
//! (B) 관리자는 로컬 작업을 잃지 않는다 — 병합 센터의 모든 연산(실패·중간
//!     "재시작" 포함)을 워크트리 무유실 검사기로 감싼다.
//!     → `WtChecker`: 연산 전 워크트리 바이트 + 도달 가능 커밋(reflog,
//!       MERGE_HEAD, ORIG_HEAD, stash 포함)을 스냅샷하고, 연산 후 모든
//!       스냅샷 커밋이 여전히 도달 가능한지, 모든 파일의 이전 내용이
//!       워크트리/커밋/reflog/인덱스 blob 어딘가에서 찾아지는지 검사한다.
//!
//! ── 발견 사항 (심각한 순) ────────────────────────────────────────────────
//!
//! [수정 완료·회귀 방지 MLOSS-1] delete_remote_branch 는 삭제 직전에 그
//!   브랜치를 fetch 해 **원격의 실제 tip** 기준으로 조상 여부를 재확인한다 —
//!   관리자의 refs 가 낡아 있어도, 마지막 fetch 이후 팀원이 push 한 새
//!   커밋이 있으면 삭제가 거부된다 (예전에는 낡은 트래킹 ref 로 가드가
//!   뚫려 팀원 커밋이 origin 에서 고아가 됐다).
//!   → s6a_delete_remote_branch_refetches_and_refuses_fresh_push
//!
//! [수정 완료·회귀 방지 MLOSS-2] abort_merge 는 중단 후 merge_in_progress 를
//!   재확인해, 병합이 남아 있는데 실패한 경우(index.lock 경합 등) Err 를
//!   돌려준다 — 예전에는 실패를 삼키고 무조건 Ok 라 상태 보고가 거짓이었다.
//!   → s2b_abort_merge_failure_is_reported
//!
//! [정상동작확인/알려진 git 의미론] 팀원이 자기 브랜치를 force-push 하면
//!   자신의 옛 커밋이 origin 에서 사라질 수 있다 — git 의 의미론이며, 앱
//!   자체는 어떤 경로로도 --force 를 쓰지 않는다(grep 증명 테스트 포함).
//!   → s5_interleaved_world_changes_mid_merge, app_code_never_force_pushes
//!
//! [정상동작확인] merge abort 는 유일하게 승인된 폐기다: 중단하면 병합 전
//!   상태로 바이트 단위 복원되고, 버려지는 것은 사용자가 명시적으로 버린
//!   해결 편집뿐이며, 팀원의 push 된 브랜치는 origin 에 그대로 남아 다시
//!   병합할 수 있다. → s2_abort_merge_is_the_only_sanctioned_discard
//!
//! [UX격차] restore_backup 은 워크트리만 되돌린다(문서화된 동작) — 병합
//!   도중 한 파일을 AI 로 해결(스테이징)한 뒤 백업을 복원하면 그 파일은
//!   더 이상 충돌 목록에 없고, 워크트리에는 마커가 보이는데 커밋에는
//!   스테이징된 해결본이 들어간다. 사용자가 직접 다시 스테이징해야 한다.
//!   → s3_auto_resolve_safety_under_checker 의 restore 구간 주석.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Mutex;

use tempfile::TempDir;

use git_companion::error::AppError;
use git_companion::git::auto::{
    auto_resolve_merge, list_backups, restore_backup, AutoResolveOptions, SideChoice,
};
use git_companion::git::fetch::fetch_target;
use git_companion::git::merge::{
    base_unpushed_count, delete_remote_branch, list_merged_remote_branches, ConflictDetail,
};
use git_companion::git::{
    abort_merge, complete_merge, list_pending_branches, merge_in_progress, push,
    remaining_conflicts, resolve_conflict, start_merge, sync_to_base, Resolution, Target,
};

/// GC_BACKUP_DIR 은 프로세스 전역 env 라 이를 만지는 테스트끼리 직렬화한다
/// (tests/git_auto_merge.rs 와 같은 패턴 — 파일이 다르면 프로세스도 다르다).
static BACKUP_LOCK: Mutex<()> = Mutex::new(());

fn backup_guard() -> std::sync::MutexGuard<'static, ()> {
    BACKUP_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ═════════════════════════════════════════════════════════════════════════════
// git 헬퍼 (tests/team_sim_races.rs 의 패턴을 따른다)
// ═════════════════════════════════════════════════════════════════════════════

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

fn git_try(dir: &Path, args: &[&str]) -> Output {
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

/// bare origin + 병합 관리자 민지의 clone. main 에는 app.txt / notes.txt
/// 한 커밋이 push 되어 있다.
fn team_origin() -> (TempDir, String, TempDir) {
    let bare = TempDir::new().unwrap();
    git(bare.path(), &["init", "--bare", "-q", "-b", "main"]);
    let url = format!("file://{}", bare.path().display());
    let mgr = TempDir::new().unwrap();
    git(mgr.path(), &["init", "-q", "-b", "main"]);
    config_user(mgr.path(), "minji");
    fs::write(mgr.path().join("app.txt"), "alpha\nbeta\ngamma\n").unwrap();
    fs::write(mgr.path().join("notes.txt"), "n1\nn2\n").unwrap();
    git(mgr.path(), &["add", "-A"]);
    git(mgr.path(), &["commit", "-q", "-m", "init"]);
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

fn bare_branch_exists(bare: &Path, branch: &str) -> bool {
    git_try(bare, &["rev-parse", "-q", "--verify", &format!("refs/heads/{branch}")])
        .status
        .success()
}

/// 팀원이 base(main)에서 브랜치를 따고 커밋 하나를 만든다. push 는 안 한다.
fn branch_with_commit(dir: &Path, branch: &str, file: &str, body: &str, msg: &str) {
    git(dir, &["checkout", "-q", "-b", branch]);
    seed_commit(dir, file, body, msg);
}

/// tests/git_auto_merge.rs 와 동일 — backup_root 가 저장소 경로에서 만드는 slug.
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

// ═════════════════════════════════════════════════════════════════════════════
// OriginLedger — 불변식 (A)
// ═════════════════════════════════════════════════════════════════════════════

/// 누군가 push 할 때마다 origin(bare)의 각 ref 에서 도달 가능한 SHA 전부를
/// 장부에 적는다. 이후 어느 시점이든 `assert_origin_intact` 로 "장부의 모든
/// SHA 가 지금도 origin ref 에서 도달 가능"을 검사할 수 있다.
#[derive(Default)]
struct OriginLedger {
    /// sha → 그 sha 가 도달 가능했던 origin ref 들.
    seen: BTreeMap<String, BTreeSet<String>>,
}

impl OriginLedger {
    fn new() -> Self {
        Self::default()
    }

    /// 모든 push 직후 호출.
    fn record(&mut self, bare: &Path) {
        let refs = git(
            bare,
            &["for-each-ref", "--format=%(refname)%09%(objectname)", "refs/heads", "refs/tags"],
        );
        for line in refs.lines() {
            let mut it = line.split('\t');
            let (Some(name), Some(tip)) = (it.next(), it.next()) else {
                continue;
            };
            for sha in git(bare, &["rev-list", tip]).lines() {
                if sha.is_empty() {
                    continue;
                }
                self.seen
                    .entry(sha.to_string())
                    .or_default()
                    .insert(name.to_string());
            }
        }
    }

    fn reachable_now(bare: &Path) -> BTreeSet<String> {
        git(bare, &["rev-list", "--all"])
            .lines()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// 장부에는 있는데 지금 origin ref 에서 도달 불가능한 SHA 들.
    fn missing(&self, bare: &Path) -> Vec<(String, BTreeSet<String>)> {
        let now = Self::reachable_now(bare);
        self.seen
            .iter()
            .filter(|(sha, _)| !now.contains(*sha))
            .map(|(s, r)| (s.clone(), r.clone()))
            .collect()
    }

    /// 불변식 (A): push 된 적 있는 커밋은 전부 여전히 도달 가능해야 한다.
    fn assert_origin_intact(&self, bare: &Path) {
        let missing = self.missing(bare);
        assert!(
            missing.is_empty(),
            "origin 에서 커밋이 유실됐다 (sha → 도달 가능했던 ref): {missing:#?}"
        );
    }

    fn branch_shas(&self, refname: &str) -> BTreeSet<String> {
        self.seen
            .iter()
            .filter(|(_, refs)| refs.contains(refname))
            .map(|(s, _)| s.clone())
            .collect()
    }

    /// delete_remote_branch 뒤: 지워진 브랜치의 장부 SHA 전부가
    /// origin/<base> 에서 도달 가능해야 한다 — 가드의 존재 이유.
    fn assert_deleted_branch_absorbed(&self, bare: &Path, branch: &str, base: &str) {
        let main: BTreeSet<String> = git(bare, &["rev-list", &format!("refs/heads/{base}")])
            .lines()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        for sha in self.branch_shas(&format!("refs/heads/{branch}")) {
            assert!(
                main.contains(&sha),
                "{branch} 삭제로 {base} 에 없는 커밋 {sha} 가 origin 에서 유실됐다"
            );
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// WtChecker — 불변식 (B): 관리자 clone 의 워크트리 무유실 검사기
// ═════════════════════════════════════════════════════════════════════════════

struct WtSnap {
    /// reflog·MERGE_HEAD·ORIG_HEAD·stash 포함, 도달 가능한 모든 커밋.
    commits: BTreeSet<String>,
    /// 워크트리 파일 (상대경로 → 바이트), .git 제외.
    files: BTreeMap<String, Vec<u8>>,
    /// 스냅샷 시점의 미해결 충돌 경로 — 이 파일들의 마커 본문은 인덱스
    /// 스테이지(:1/:2/:3)에서 재생성 가능하므로 blob 부재가 유실이 아니다.
    conflicted: BTreeSet<String>,
}

struct WtChecker {
    repo: PathBuf,
}

impl WtChecker {
    fn new(repo: &Path) -> Self {
        Self { repo: repo.to_path_buf() }
    }

    fn special_roots(&self) -> Vec<String> {
        let mut out = Vec::new();
        for r in ["MERGE_HEAD", "ORIG_HEAD"] {
            let o = git_try(&self.repo, &["rev-parse", "-q", "--verify", &format!("{r}^{{commit}}")]);
            if o.status.success() {
                let sha = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !sha.is_empty() {
                    out.push(sha);
                }
            }
        }
        out
    }

    fn commits(&self) -> BTreeSet<String> {
        let mut args: Vec<String> = vec!["rev-list".into(), "--all".into(), "--reflog".into()];
        args.extend(self.special_roots());
        let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        git(&self.repo, &argv)
            .lines()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// 지금 이 저장소에서 "찾을 수 있는" blob 전부: 커밋/reflog 도달 가능한
    /// 객체 + 인덱스(스테이지 0~3)의 blob.
    fn findable_blobs(&self) -> BTreeSet<String> {
        let mut args: Vec<String> =
            vec!["rev-list".into(), "--objects".into(), "--all".into(), "--reflog".into()];
        args.extend(self.special_roots());
        let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let mut out: BTreeSet<String> = git(&self.repo, &argv)
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .map(str::to_string)
            .collect();
        for line in git(&self.repo, &["ls-files", "-s"]).lines() {
            if let Some(sha) = line.split_whitespace().nth(1) {
                out.insert(sha.to_string());
            }
        }
        // 자동 해결 백업(GC_BACKUP_DIR)도 정당한 보존 장소다 — restore 로
        // 살아난 마커 본문을 사용자가 다시 덮어써도 백업에 남아 있으면
        // 유실이 아니다.
        if let Some(dir) = std::env::var_os("GC_BACKUP_DIR") {
            fn walk_any(dir: &Path, out: &mut Vec<PathBuf>) {
                let Ok(rd) = fs::read_dir(dir) else { return };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        walk_any(&p, out);
                    } else {
                        out.push(p);
                    }
                }
            }
            let mut files = Vec::new();
            walk_any(&PathBuf::from(dir).join(backup_slug(&self.repo)), &mut files);
            for f in files {
                if let Ok(bytes) = fs::read(&f) {
                    out.insert(self.hash_bytes(&bytes));
                }
            }
        }
        out
    }

    fn worktree_files(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for e in fs::read_dir(dir).unwrap() {
                let e = e.unwrap();
                if e.file_name() == ".git" {
                    continue;
                }
                let p = e.path();
                if p.is_dir() {
                    walk(root, &p, out);
                } else {
                    let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                    out.insert(rel, fs::read(&p).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.repo, &self.repo, &mut out);
        out
    }

    fn conflicted(&self) -> BTreeSet<String> {
        git(&self.repo, &["diff", "--name-only", "--diff-filter=U"])
            .lines()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// 스냅샷 시점에 HEAD 와 다른(진행 중인) 경로들 — abort 가 승인 하에
    /// 폐기하는 대상이다.
    fn dirty_paths(&self) -> Vec<String> {
        git(&self.repo, &["status", "--porcelain"])
            .lines()
            .filter(|l| l.len() > 3)
            .map(|l| l[3..].trim().trim_matches('"').to_string())
            .collect()
    }

    fn hash_bytes(&self, bytes: &[u8]) -> String {
        let mut child = std::process::Command::new("git")
            .args(["hash-object", "--stdin"])
            .current_dir(&self.repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        let out = child.wait_with_output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn snapshot(&self) -> WtSnap {
        WtSnap {
            commits: self.commits(),
            files: self.worktree_files(),
            conflicted: self.conflicted(),
        }
    }

    /// 연산 후 검증. `sanctioned` 는 이 연산이 폐기해도 되는(사용자가 명시적
    /// 으로 승인한) 경로 — abort_merge 가 유일한 정당한 사용처다.
    fn verify(&self, snap: &WtSnap, op: &str, sanctioned: &[String]) {
        // 1. 커밋: 스냅샷의 모든 커밋이 여전히 도달 가능해야 한다.
        let after = self.commits();
        let lost: Vec<&String> = snap.commits.difference(&after).collect();
        assert!(
            lost.is_empty(),
            "[{op}] 관리자 clone 에서 커밋 유실 — 연산 전에 도달 가능했던 커밋이 사라졌다: {lost:?}"
        );
        // 2. 파일: 이전 내용이 어딘가에서 찾아져야 한다.
        let now = self.worktree_files();
        let mut findable: Option<BTreeSet<String>> = None;
        for (path, bytes) in &snap.files {
            if now.get(path).map(|b| b == bytes).unwrap_or(false) {
                continue; // 그대로 있다.
            }
            let f = findable.get_or_insert_with(|| self.findable_blobs());
            let blob = self.hash_bytes(bytes);
            if f.contains(&blob) {
                continue; // 커밋/reflog/인덱스에 남아 있다.
            }
            if snap.conflicted.contains(path) {
                // 충돌 마커 본문 — :1/:2/:3 스테이지(또는 그 스테이지가 가리
                // 키던 커밋 blob)에서 재생성 가능. ours/theirs/manual 선택으로
                // 덮이는 것은 사용자의 명시적 결정이다.
                continue;
            }
            if sanctioned.iter().any(|s| s == path) {
                continue; // 문서화·승인된 폐기 (merge abort).
            }
            panic!(
                "[{op}] 관리자 작업물 유실: {path} 의 이전 내용(blob {blob}, {}바이트)을 \
                 워크트리/커밋/reflog/인덱스 어디에서도 찾을 수 없다",
                bytes.len()
            );
        }
    }
}

/// 모든 `git_companion::git::*` 관리자 연산을 이 래퍼로 감싼다.
fn checked<T>(chk: &WtChecker, op: &str, sanctioned: &[String], f: impl FnOnce() -> T) -> T {
    let snap = chk.snapshot();
    let out = f();
    chk.verify(&snap, op, sanctioned);
    out
}

fn assert_tree_identical(before: &BTreeMap<String, Vec<u8>>, after: &BTreeMap<String, Vec<u8>>) {
    let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    for k in keys {
        match (before.get(k), after.get(k)) {
            (Some(a), Some(b)) => assert!(a == b, "{k}: 내용이 병합 전과 다르다"),
            (Some(_), None) => panic!("{k}: 병합 전에는 있었는데 사라졌다"),
            (None, Some(_)) => panic!("{k}: 병합 전에는 없던 파일이 남았다"),
            (None, None) => unreachable!(),
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 1 — 관리자의 평범한 한 주, 두 불변식 아래서
// ═════════════════════════════════════════════════════════════════════════════

/// 팀원 5명의 브랜치가 도착한다. 깨끗한 병합 3개, 충돌 병합 2개
/// (theirs / ours+manual 혼합), 매 병합마다 push, 끝나면
/// delete_remote_branch 로 정리. 모든 push/병합/삭제 뒤에 장부와 검사기가
/// 돈다. 마지막에 모든 팀원의 push 커밋이 origin/main 에서 도달 가능해야
/// 한다. [정상동작확인 — 해결 선택(ours/theirs)이 상대 *내용*을 덮는 것은
/// 사람의 결정이고, 커밋 자체는 병합 커밋의 부모로 영원히 남는다.]
#[test]
fn s1_canonical_manager_week_under_both_invariants() {
    let (bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());
    let chk = WtChecker::new(mgr.path());
    let mut ledger = OriginLedger::new();
    ledger.record(bare.path()); // 관리자의 초기 push

    // 팀원 5명 — 전원 초기 main 에서 clone.
    let jisu = person(&url, "jisu");
    let haneul = person(&url, "haneul");
    let doyun = person(&url, "doyun");
    let seoyeon = person(&url, "seoyeon");
    let junho = person(&url, "junho");

    branch_with_commit(jisu.path(), "feature/jisu", "jisu.txt", "j\n", "feat jisu");
    branch_with_commit(haneul.path(), "feature/haneul", "haneul.txt", "h\n", "feat haneul");
    branch_with_commit(
        doyun.path(),
        "feature/doyun",
        "app.txt",
        "alpha-doyun\nbeta\ngamma\n",
        "feat doyun",
    );
    branch_with_commit(
        seoyeon.path(),
        "feature/seoyeon",
        "app.txt",
        "alpha-seoyeon\nbeta\ngamma\n",
        "feat seoyeon app",
    );
    seed_commit(seoyeon.path(), "notes.txt", "n1-seoyeon\nn2\n", "feat seoyeon notes");
    branch_with_commit(
        junho.path(),
        "feature/junho",
        "app.txt",
        "alpha-junho\nbeta\ngamma\n",
        "feat junho app",
    );
    seed_commit(junho.path(), "notes.txt", "n1-junho\nn2\n", "feat junho notes");
    seed_commit(junho.path(), "junho.txt", "jh\n", "feat junho file");

    let members: [(&str, &TempDir); 5] = [
        ("feature/jisu", &jisu),
        ("feature/haneul", &haneul),
        ("feature/doyun", &doyun),
        ("feature/seoyeon", &seoyeon),
        ("feature/junho", &junho),
    ];
    let mut tips: BTreeMap<String, String> = BTreeMap::new();
    for (branch, dir) in &members {
        git(dir.path(), &["push", "-q", "origin", branch]);
        tips.insert(branch.to_string(), head_sha(dir.path(), branch));
        ledger.record(bare.path());
        ledger.assert_origin_intact(bare.path());
    }

    // 관리자: 가져오기 → 대기 목록에 5개.
    checked(&chk, "fetch", &[], || fetch_target(&mgr_t, "origin")).unwrap();
    let pending = checked(&chk, "list_pending", &[], || {
        list_pending_branches(&mgr_t, "origin", "main")
    })
    .unwrap();
    assert_eq!(pending.len(), 5, "{pending:?}");
    ledger.assert_origin_intact(bare.path());

    let push_main = |chk: &WtChecker, ledger: &mut OriginLedger| {
        let out = checked(chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap();
        assert!(out.ok, "push 실패: {}", out.message);
        ledger.record(bare.path());
        ledger.assert_origin_intact(bare.path());
    };

    // 1~3. 깨끗한 병합: jisu, haneul, doyun.
    for b in ["feature/jisu", "feature/haneul", "feature/doyun"] {
        let out = checked(&chk, &format!("start_merge({b})"), &[], || {
            start_merge(&mgr_t, &format!("origin/{b}"), "main", "origin", Some(tips[b].as_str()))
        })
        .unwrap();
        assert!(out.ok && !out.conflicted, "{b} 는 깨끗해야: {}", out.message);
        ledger.assert_origin_intact(bare.path());
        push_main(&chk, &mut ledger);
    }

    // 4. seoyeon: app.txt 충돌 → theirs 로 해결.
    let out = checked(&chk, "start_merge(seoyeon)", &[], || {
        start_merge(
            &mgr_t,
            "origin/feature/seoyeon",
            "main",
            "origin",
            Some(tips["feature/seoyeon"].as_str()),
        )
    })
    .unwrap();
    assert!(out.conflicted);
    assert_eq!(out.conflicted_files, vec!["app.txt".to_string()]);
    let rem = checked(&chk, "resolve(app.txt, theirs)", &[], || {
        resolve_conflict(&mgr_t, "app.txt", &Resolution::Theirs)
    })
    .unwrap();
    assert!(rem.is_empty());
    let done = checked(&chk, "complete_merge(seoyeon)", &[], || {
        complete_merge(&mgr_t, Some("feature/seoyeon 브랜치 병합"))
    })
    .unwrap();
    assert!(done.ok, "{}", done.message);
    ledger.assert_origin_intact(bare.path());
    push_main(&chk, &mut ledger);

    // 5. junho: app.txt + notes.txt 충돌 → ours + manual 혼합.
    let out = checked(&chk, "start_merge(junho)", &[], || {
        start_merge(
            &mgr_t,
            "origin/feature/junho",
            "main",
            "origin",
            Some(tips["feature/junho"].as_str()),
        )
    })
    .unwrap();
    assert!(out.conflicted);
    let mut cf = out.conflicted_files.clone();
    cf.sort();
    assert_eq!(cf, vec!["app.txt".to_string(), "notes.txt".to_string()]);
    let rem = checked(&chk, "resolve(app.txt, ours)", &[], || {
        resolve_conflict(&mgr_t, "app.txt", &Resolution::Ours)
    })
    .unwrap();
    assert_eq!(rem, vec!["notes.txt".to_string()]);
    let rem = checked(&chk, "resolve(notes.txt, manual)", &[], || {
        resolve_conflict(
            &mgr_t,
            "notes.txt",
            &Resolution::Manual { content: "n1-united\nn2\n".into() },
        )
    })
    .unwrap();
    assert!(rem.is_empty());
    let done = checked(&chk, "complete_merge(junho)", &[], || complete_merge(&mgr_t, None)).unwrap();
    assert!(done.ok, "{}", done.message);
    ledger.assert_origin_intact(bare.path());
    push_main(&chk, &mut ledger);

    // 해결 결과 확인 — theirs(seoyeon) 뒤 ours(=seoyeon 유지) + manual.
    assert_eq!(
        fs::read_to_string(mgr.path().join("app.txt")).unwrap(),
        "alpha-seoyeon\nbeta\ngamma\n"
    );
    assert_eq!(fs::read_to_string(mgr.path().join("notes.txt")).unwrap(), "n1-united\nn2\n");

    // 모든 팀원의 push 커밋이 origin/main 에서 도달 가능하다.
    for (branch, tip) in &tips {
        assert!(
            is_ancestor(bare.path(), tip, "refs/heads/main"),
            "{branch} 의 커밋 {tip} 이 origin/main 에 없다"
        );
    }
    let pending = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    assert!(pending.is_empty(), "전부 병합됐어야: {pending:?}");

    // 정리 — 병합 끝난 원격 브랜치 삭제. 삭제마다 장부가 "main 에 흡수됐는지"
    // 검사한다.
    let merged = checked(&chk, "list_merged", &[], || {
        list_merged_remote_branches(&mgr_t, "origin", "main")
    })
    .unwrap();
    let names: BTreeSet<String> = merged.iter().map(|b| b.short_name.clone()).collect();
    for (branch, _) in &members {
        assert!(names.contains(*branch), "{branch} 는 정리 후보여야 한다: {names:?}");
    }
    for (branch, _) in &members {
        checked(&chk, &format!("delete_remote_branch({branch})"), &[], || {
            delete_remote_branch(&mgr_t, "origin", "main", branch)
        })
        .unwrap();
        assert!(!bare_branch_exists(bare.path(), branch));
        ledger.assert_deleted_branch_absorbed(bare.path(), branch, "main");
        ledger.assert_origin_intact(bare.path());
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 2 — merge abort 는 유일하게 승인된 폐기다
// ═════════════════════════════════════════════════════════════════════════════

/// 병합 시작 → 충돌 → 부분 해결(한 파일 theirs, 한 파일 손편집) → 중단.
/// 병합 전 상태가 바이트 단위로 복원되고, origin 은 손대지 않으며, 사라지는
/// 것은 사용자가 명시적으로 버린 해결 편집뿐이다. 팀원의 브랜치는 origin 에
/// 그대로 남아 다시 병합된다. [정상동작확인]
#[test]
fn s2_abort_merge_is_the_only_sanctioned_discard() {
    let (bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());
    let chk = WtChecker::new(mgr.path());
    let mut ledger = OriginLedger::new();
    ledger.record(bare.path());

    let member = person(&url, "jisu");
    branch_with_commit(
        member.path(),
        "feature/hot",
        "app.txt",
        "alpha-hot\nbeta\ngamma\n",
        "feat hot app",
    );
    seed_commit(member.path(), "notes.txt", "n1-hot\nn2\n", "feat hot notes");
    git(member.path(), &["push", "-q", "origin", "feature/hot"]);
    let member_tip = head_sha(member.path(), "feature/hot");
    ledger.record(bare.path());

    // 관리자 main 에 겹치는 편집 → 두 파일 다 충돌.
    seed_commit(mgr.path(), "app.txt", "alpha-main\nbeta\ngamma\n", "main app");
    seed_commit(mgr.path(), "notes.txt", "n1-main\nn2\n", "main notes");
    let out = checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap();
    assert!(out.ok);
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());

    // 병합 전 상태를 통째로 캡처 (워크트리 바이트 + origin refs).
    let pre_files = chk.worktree_files();
    let pre_origin = git(bare.path(), &["for-each-ref"]);

    let out = checked(&chk, "start_merge(hot)", &[], || {
        start_merge(&mgr_t, "origin/feature/hot", "main", "origin", Some(member_tip.as_str()))
    })
    .unwrap();
    assert!(out.conflicted);
    let mut cf = out.conflicted_files.clone();
    cf.sort();
    assert_eq!(cf, vec!["app.txt".to_string(), "notes.txt".to_string()]);

    // 부분 해결: app.txt 는 theirs, notes.txt 는 편집기에서 반쯤 손봄.
    checked(&chk, "resolve(app.txt, theirs)", &[], || {
        resolve_conflict(&mgr_t, "app.txt", &Resolution::Theirs)
    })
    .unwrap();
    fs::write(mgr.path().join("notes.txt"), "n1-half-done-resolution\n").unwrap();

    // 중단 — 지금 진행 중이던 경로들만 승인된 폐기 대상이다.
    let sanctioned = chk.dirty_paths();
    checked(&chk, "abort_merge", &sanctioned, || abort_merge(&mgr_t)).unwrap();

    assert!(!merge_in_progress(&mgr_t).unwrap());
    assert!(remaining_conflicts(&mgr_t).unwrap().is_empty());

    // 병합 전 상태가 바이트 단위로 복원됐다 — 버려진 건 해결 편집뿐이다.
    let post_files = chk.worktree_files();
    assert_tree_identical(&pre_files, &post_files);
    assert!(
        !fs::read_to_string(mgr.path().join("notes.txt")).unwrap().contains("half-done"),
        "버린 손편집은 사라져야 한다 (문서화·승인된 폐기)"
    );

    // origin 은 손끝 하나 안 댔다.
    assert_eq!(git(bare.path(), &["for-each-ref"]), pre_origin);
    ledger.assert_origin_intact(bare.path());

    // 팀원의 브랜치는 그대로 남아 다시 병합할 수 있다.
    let out = checked(&chk, "start_merge(hot, 재시도)", &[], || {
        start_merge(&mgr_t, "origin/feature/hot", "main", "origin", Some(member_tip.as_str()))
    })
    .unwrap();
    assert!(out.conflicted, "같은 충돌이 다시 나야 한다");
    checked(&chk, "resolve(app.txt)", &[], || {
        resolve_conflict(&mgr_t, "app.txt", &Resolution::Theirs)
    })
    .unwrap();
    checked(&chk, "resolve(notes.txt)", &[], || {
        resolve_conflict(
            &mgr_t,
            "notes.txt",
            &Resolution::Manual { content: "n1-final\nn2\n".into() },
        )
    })
    .unwrap();
    let done = checked(&chk, "complete_merge", &[], || complete_merge(&mgr_t, None)).unwrap();
    assert!(done.ok);
    let out = checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap();
    assert!(out.ok);
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());
    assert!(is_ancestor(bare.path(), &member_tip, "refs/heads/main"));
}

/// 회귀 방지(MLOSS-2): index.lock 등으로 `git merge --abort` 가 실패하면
/// abort_merge 는 Err 를 돌려준다 — "중단됨"이라는 거짓 보고 금지.
#[test]
fn s2b_abort_merge_failure_is_reported() {
    let (_bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());
    let chk = WtChecker::new(mgr.path());

    let member = person(&url, "jisu");
    branch_with_commit(
        member.path(),
        "feature/x",
        "app.txt",
        "alpha-x\nbeta\ngamma\n",
        "feat x",
    );
    git(member.path(), &["push", "-q", "origin", "feature/x"]);
    seed_commit(mgr.path(), "app.txt", "alpha-main\nbeta\ngamma\n", "main edit");
    let out = checked(&chk, "start_merge", &[], || {
        start_merge(&mgr_t, "origin/feature/x", "main", "origin", None)
    })
    .unwrap();
    assert!(out.conflicted);

    // 다른 git 프로세스가 index.lock 을 잡고 있다.
    let lock = mgr.path().join(".git/index.lock");
    fs::write(&lock, "").unwrap();

    let res = checked(&chk, "abort_merge(잠김)", &[], || abort_merge(&mgr_t));
    let err = res.expect_err("중단 실패는 Err 로 보고돼야 한다");
    assert!(err.to_string().contains("병합 중단 실패"), "{err}");
    assert!(
        merge_in_progress(&mgr_t).unwrap(),
        "실패했으니 MERGE_HEAD 는 그대로 남는다 — 상태 보고와 실제가 일치"
    );

    // 락이 풀리면 정상적으로 중단된다. 작업물은 그동안 그대로였다.
    fs::remove_file(&lock).unwrap();
    let sanctioned = chk.dirty_paths();
    checked(&chk, "abort_merge(재시도)", &sanctioned, || abort_merge(&mgr_t)).unwrap();
    assert!(!merge_in_progress(&mgr_t).unwrap());
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 3 — 자동 해결(AI)의 안전성, 검사기 아래서
// ═════════════════════════════════════════════════════════════════════════════

/// 충돌 파일 3개에 대해 AI 가 쓰레기/유효/실패를 섞어 내놓는다. 백업은 항상
/// 해결 전 바이트 전부를 담아야 하고, restore_backup 은 바이트 단위로
/// 되돌리며, 자동 해결이 커밋되면 양쪽 부모가 모두 도달 가능해야 한다.
/// 양쪽이 다 고친 파일에서 AI 가 실패하면 아무것도 커밋되지 않는다.
#[test]
fn s3_auto_resolve_safety_under_checker() {
    let _guard = backup_guard();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("GC_BACKUP_DIR", tmp.path().join("backups"));

    let (bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());
    let chk = WtChecker::new(mgr.path());
    let mut ledger = OriginLedger::new();

    // 공유 파일 3개를 main 에 깔고 push.
    for (f, b) in [("f1.txt", "f1-base\n"), ("f2.txt", "f2-base\n"), ("f3.txt", "f3-base\n")] {
        seed_commit(mgr.path(), f, b, &format!("add {f}"));
    }
    let out = push(&mgr_t, Some("main"), None).unwrap();
    assert!(out.ok);
    ledger.record(bare.path());

    // 팀원: 세 파일 모두 수정.
    let member = person(&url, "haneul");
    git(member.path(), &["checkout", "-q", "-b", "feature/ai"]);
    for (f, b) in [("f1.txt", "f1-member\n"), ("f2.txt", "f2-member\n"), ("f3.txt", "f3-member\n")] {
        seed_commit(member.path(), f, b, &format!("member {f}"));
    }
    git(member.path(), &["push", "-q", "origin", "feature/ai"]);
    let member_tip = head_sha(member.path(), "feature/ai");
    let member_first = git(member.path(), &["rev-list", "feature/ai", "--not", "origin/main"])
        .lines()
        .last()
        .unwrap()
        .to_string();
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());

    // 관리자도 세 파일 모두 수정 (양쪽 변경 → 자동 한쪽 선택 금지 대상).
    for (f, b) in [("f1.txt", "f1-mgr\n"), ("f2.txt", "f2-mgr\n"), ("f3.txt", "f3-mgr\n")] {
        seed_commit(mgr.path(), f, b, &format!("mgr {f}"));
    }
    let out = checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap();
    assert!(out.ok);
    ledger.record(bare.path());
    let pre_merge_main = head_sha(mgr.path(), "main");

    let out = checked(&chk, "start_merge(ai)", &[], || {
        start_merge(&mgr_t, "origin/feature/ai", "main", "origin", Some(member_tip.as_str()))
    })
    .unwrap();
    assert!(out.conflicted);
    let mut cf = out.conflicted_files.clone();
    cf.sort();
    assert_eq!(cf, vec!["f1.txt", "f2.txt", "f3.txt"]);

    // 해결 전 마커 본문 캡처 — 백업의 기준값.
    let pre: BTreeMap<&str, Vec<u8>> = ["f1.txt", "f2.txt", "f3.txt"]
        .iter()
        .map(|f| (*f, fs::read(mgr.path().join(f)).unwrap()))
        .collect();
    let backup_dir = tmp.path().join("backups").join(backup_slug(mgr.path()));
    let assert_backup_exact = |id: &str| {
        for (f, bytes) in &pre {
            assert_eq!(
                &fs::read(backup_dir.join(id).join(f)).unwrap(),
                bytes,
                "백업 {id} 의 {f} 는 해결 전 바이트와 정확히 같아야 한다"
            );
        }
    };

    // ── 1차: AI 전멸 — 양쪽 변경 파일이므로 아무것도 해결/커밋되면 안 된다.
    let report = checked(&chk, "auto_resolve(전멸)", &[], || {
        auto_resolve_merge(&mgr_t, &AutoResolveOptions::default(), |_d: &ConflictDetail| {
            Err(AppError::Config("AI 응답 없음".into()))
        })
    })
    .unwrap();
    assert!(report.resolved.is_empty(), "{:?}", report.resolved);
    assert_eq!(report.remaining, vec!["f1.txt", "f2.txt", "f3.txt"]);
    assert!(!report.committed, "양쪽 변경 파일에서 AI 실패 → 커밋 금지");
    assert_backup_exact(&report.backup_id.expect("백업은 항상 먼저"));
    assert!(merge_in_progress(&mgr_t).unwrap());
    assert_eq!(head_sha(mgr.path(), "HEAD"), pre_merge_main, "HEAD 가 움직이면 안 된다");
    ledger.assert_origin_intact(bare.path());

    // ── 2차: 혼합 — f1 유효, f2 마커 쓰레기(거부돼야), f3 실패.
    let report = checked(&chk, "auto_resolve(혼합)", &[], || {
        auto_resolve_merge(&mgr_t, &AutoResolveOptions::default(), |d: &ConflictDetail| {
            match d.path.as_str() {
                "f1.txt" => Ok("f1 resolved by ai\n".to_string()),
                "f2.txt" => Ok("<<<<<<< HEAD\ngarbage\n=======\nmore\n>>>>>>> x\n".to_string()),
                _ => Err(AppError::Config("AI 시간 초과".into())),
            }
        })
    })
    .unwrap();
    assert_eq!(report.resolved.len(), 1);
    assert_eq!(report.resolved[0].path, "f1.txt");
    assert_eq!(report.resolved[0].method, "ai");
    assert_eq!(report.remaining, vec!["f2.txt", "f3.txt"]);
    assert!(!report.committed);
    let backup_b = report.backup_id.expect("2차 백업");
    assert_backup_exact(&backup_b);
    assert!(
        !fs::read_to_string(mgr.path().join("f2.txt")).unwrap().contains("garbage"),
        "마커 든 AI 결과는 절대 파일에 쓰이면 안 된다"
    );

    // restore_backup: 바이트 단위 복원, 복원 대상 외에는 아무것도 안 바뀐다.
    let before_restore = chk.worktree_files();
    let n = checked(&chk, "restore_backup", &[], || restore_backup(&mgr_t, &backup_b)).unwrap();
    assert_eq!(n, 3);
    for (f, bytes) in &pre {
        assert_eq!(&fs::read(mgr.path().join(f)).unwrap(), bytes, "{f} 복원은 바이트 단위");
    }
    for (path, bytes) in &before_restore {
        if *path == "f1.txt" {
            continue; // 유일하게 되돌아간 파일 (2차에서 해결됐던 것).
        }
        assert_eq!(&fs::read(mgr.path().join(path)).unwrap(), bytes, "{path} 는 안 바뀌어야");
    }
    // [UX격차 기록] 복원은 워크트리만 되돌린다(문서화된 동작): f1 은 2차에서
    // 스테이징돼 더는 충돌 목록에 없다 — 지금 커밋하면 워크트리(마커)가 아닌
    // 스테이징된 AI 본문이 들어간다. 사용자가 직접 다시 확정해야 한다.
    assert_eq!(remaining_conflicts(&mgr_t).unwrap(), vec!["f2.txt", "f3.txt"]);
    assert!(merge_in_progress(&mgr_t).unwrap());
    assert_eq!(head_sha(mgr.path(), "HEAD"), pre_merge_main, "여전히 아무것도 커밋 안 됨");

    // 사용자가 f1 을 검토하고 다시 확정한다 (Manual 은 쓰기+스테이징).
    checked(&chk, "resolve(f1, manual 재확정)", &[], || {
        resolve_conflict(&mgr_t, "f1.txt", &Resolution::Manual { content: "f1 resolved by ai\n".into() })
    })
    .unwrap();

    // ── 3차: 남은 두 파일에 유효한 AI → 전부 해결되고 커밋된다.
    let report = checked(&chk, "auto_resolve(유효)", &[], || {
        auto_resolve_merge(&mgr_t, &AutoResolveOptions::default(), |d: &ConflictDetail| {
            Ok(format!("{} merged by ai\n", d.path))
        })
    })
    .unwrap();
    assert!(report.committed, "{}", report.message);
    assert!(report.remaining.is_empty());
    assert!(!merge_in_progress(&mgr_t).unwrap());

    // 커밋된 자동 해결은 양쪽 부모를 모두 보존한다 — 팀원의 원본 커밋이
    // 병합 커밋에서 도달 가능하다. 아무것도 "흡수돼 사라지지" 않았다.
    let parents: Vec<String> = git(mgr.path(), &["log", "-1", "--pretty=%P"])
        .split_whitespace()
        .map(str::to_string)
        .collect();
    assert_eq!(parents.len(), 2, "병합 커밋은 부모 둘");
    assert_eq!(parents[0], pre_merge_main);
    assert_eq!(parents[1], member_tip);
    assert!(is_ancestor(mgr.path(), &member_first, "HEAD"), "팀원의 원본 커밋 도달 가능");

    let out = checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap();
    assert!(out.ok);
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());
    assert!(is_ancestor(bare.path(), &member_tip, "refs/heads/main"));

    // 세 번의 실행 = 세 개의 백업.
    assert_eq!(list_backups(&mgr_t).unwrap().len(), 3);
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 4 — 최악의 순간에 push 가 실패한다
// ═════════════════════════════════════════════════════════════════════════════

/// complete_merge 직후 push 가 실패한다(원격이 잠시 사라짐 — bare 디렉터리를
/// 치웠다가 되돌린다). 병합 커밋은 로컬에 남고, base_unpushed_count 는
/// 진실하며, 대기 목록은 그 브랜치를 "다시 병합"이 아니라 "푸시 대기"로
/// 표시하고, 재시도 push 가 성공하며, origin 은 내내 무결하다. [정상동작확인]
#[test]
fn s4_push_failure_at_the_worst_moment() {
    let (bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());
    let chk = WtChecker::new(mgr.path());
    let mut ledger = OriginLedger::new();
    ledger.record(bare.path());

    let member = person(&url, "doyun");
    branch_with_commit(
        member.path(),
        "feature/pay",
        "app.txt",
        "alpha-pay\nbeta\ngamma\n",
        "feat pay",
    );
    git(member.path(), &["push", "-q", "origin", "feature/pay"]);
    let member_tip = head_sha(member.path(), "feature/pay");
    ledger.record(bare.path());

    seed_commit(mgr.path(), "app.txt", "alpha-main\nbeta\ngamma\n", "main edit");
    assert!(checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap().ok);
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());

    let out = checked(&chk, "start_merge(pay)", &[], || {
        start_merge(&mgr_t, "origin/feature/pay", "main", "origin", Some(member_tip.as_str()))
    })
    .unwrap();
    assert!(out.conflicted);
    checked(&chk, "resolve(manual)", &[], || {
        resolve_conflict(
            &mgr_t,
            "app.txt",
            &Resolution::Manual { content: "alpha-agreed\nbeta\ngamma\n".into() },
        )
    })
    .unwrap();
    let done = checked(&chk, "complete_merge", &[], || complete_merge(&mgr_t, None)).unwrap();
    assert!(done.ok);
    let merged_head = head_sha(mgr.path(), "HEAD");

    // 최악의 순간: 원격이 사라진다 (디렉터리를 통째로 치운다).
    let jail = TempDir::new().unwrap();
    let hidden = jail.path().join("bare-away");
    fs::rename(bare.path(), &hidden).unwrap();

    let out = checked(&chk, "push(원격 없음)", &[], || push(&mgr_t, Some("main"), None)).unwrap();
    assert!(!out.ok, "원격이 없으니 실패해야 한다: {}", out.message);

    // 병합 커밋은 로컬에 그대로 남았고, 카운트는 진실하다
    // (팀원 커밋 1 + 병합 커밋 1 = 원격 main 보다 2 앞섬).
    assert_eq!(head_sha(mgr.path(), "HEAD"), merged_head);
    assert_eq!(base_unpushed_count(&mgr_t, "origin", "main").unwrap(), 2);

    // 대기 목록은 "다시 병합하라"고 세우지 않는다 — merged_locally 플래그.
    let pending = checked(&chk, "list_pending(원격 없음)", &[], || {
        list_pending_branches(&mgr_t, "origin", "main")
    })
    .unwrap();
    let pay = pending
        .iter()
        .find(|b| b.short_name == "feature/pay")
        .expect("아직 원격 main 에 없으니 목록에는 남는다");
    assert!(pay.merged_locally, "'푸시 대기'로 보여야지 '병합 대기'가 아니다");

    // 원격 복구 → 재시도 성공.
    fs::rename(&hidden, bare.path()).unwrap();
    let out = checked(&chk, "push(재시도)", &[], || push(&mgr_t, Some("main"), None)).unwrap();
    assert!(out.ok, "{}", out.message);
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());
    assert!(is_ancestor(bare.path(), &member_tip, "refs/heads/main"));
    assert_eq!(base_unpushed_count(&mgr_t, "origin", "main").unwrap(), 0);
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 5 — 병합 도중 세상이 바뀐다 (새 push / force-push)
// ═════════════════════════════════════════════════════════════════════════════

/// 관리자가 충돌 해결 중일 때 팀원들이 브랜치 2개를 더 push 하고 한 명은
/// force-push 한다. 원래 병합은 무사히 끝나고, 대기 목록은 새 진실을 보여
/// 주며, expected_sha 가드가 낡은 검토 기준의 병합을 막는다.
/// [정상동작확인/알려진 git 의미론] force-push 는 그 팀원 *자신의* 브랜치의
/// 옛 커밋을 origin 에서 떨어뜨릴 수 있다 — 정확히 그 커밋 하나만 사라졌고
/// 다른 어떤 ref 의 커밋도 다치지 않았음을 장부로 측정해 분류한다. 앱은
/// 스스로 force-push 하지 않는다 (app_code_never_force_pushes 로 grep 증명).
#[test]
fn s5_interleaved_world_changes_mid_merge() {
    let (bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());
    let chk = WtChecker::new(mgr.path());
    let mut ledger = OriginLedger::new();
    ledger.record(bare.path());

    // 팀원 A: 충돌 브랜치. B, C: 나중에 등장.
    let a = person(&url, "memberA");
    let b = person(&url, "memberB");
    let c = person(&url, "memberC");
    branch_with_commit(a.path(), "feature/a", "app.txt", "alpha-a\nbeta\ngamma\n", "feat a");
    git(a.path(), &["push", "-q", "origin", "feature/a"]);
    let a_tip = head_sha(a.path(), "feature/a");
    ledger.record(bare.path());

    seed_commit(mgr.path(), "app.txt", "alpha-m\nbeta\ngamma\n", "main edit");
    assert!(checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap().ok);
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());

    // 관리자: 병합 시작 → 충돌 → 해결 도중이다.
    let out = checked(&chk, "start_merge(a)", &[], || {
        start_merge(&mgr_t, "origin/feature/a", "main", "origin", Some(a_tip.as_str()))
    })
    .unwrap();
    assert!(out.conflicted);

    // 그 사이 세상이 움직인다: B push → C push → B force-push.
    branch_with_commit(b.path(), "feature/b", "b.txt", "b-v1\n", "feat b");
    git(b.path(), &["push", "-q", "origin", "feature/b"]);
    let b_old = head_sha(b.path(), "feature/b");
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());

    branch_with_commit(c.path(), "feature/c", "c.txt", "c\n", "feat c");
    git(c.path(), &["push", "-q", "origin", "feature/c"]);
    let c_tip = head_sha(c.path(), "feature/c");
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());

    // B 가 자기 브랜치를 고쳐 force-push (터미널에서 — 앱 밖 행동).
    fs::write(b.path().join("b.txt"), "b-v2\n").unwrap();
    git(b.path(), &["commit", "-q", "-a", "--amend", "-m", "feat b (amended)"]);
    git(b.path(), &["push", "-q", "--force", "origin", "feature/b"]);
    let b_new = head_sha(b.path(), "feature/b");
    ledger.record(bare.path());

    // 정직한 측정: 장부에서 사라진 것은 정확히 B 의 옛 tip 하나이고, 그
    // 커밋이 도달 가능했던 ref 는 B 자신의 브랜치뿐이다.
    // [정상동작확인/알려진 git 의미론 — 자기 브랜치의 force-push]
    let missing = ledger.missing(bare.path());
    assert_eq!(missing.len(), 1, "{missing:#?}");
    assert_eq!(missing[0].0, b_old);
    assert_eq!(
        missing[0].1,
        BTreeSet::from(["refs/heads/feature/b".to_string()]),
        "옛 커밋은 오직 B 자신의 브랜치에서만 도달 가능했다"
    );

    // 관리자는 아무 일 없다는 듯 원래 병합을 끝낸다.
    checked(&chk, "resolve(manual)", &[], || {
        resolve_conflict(
            &mgr_t,
            "app.txt",
            &Resolution::Manual { content: "alpha-united\nbeta\ngamma\n".into() },
        )
    })
    .unwrap();
    let done = checked(&chk, "complete_merge(a)", &[], || complete_merge(&mgr_t, None)).unwrap();
    assert!(done.ok);
    assert!(checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap().ok);
    ledger.record(bare.path());
    assert_eq!(ledger.missing(bare.path()).len(), 1, "여전히 b_old 하나뿐");

    // 대기 목록은 새 진실을 보여 준다: b(새 tip), c — a 는 병합됐으니 없다.
    checked(&chk, "fetch", &[], || fetch_target(&mgr_t, "origin")).unwrap();
    let pending = checked(&chk, "list_pending", &[], || {
        list_pending_branches(&mgr_t, "origin", "main")
    })
    .unwrap();
    assert!(pending.iter().all(|p| p.short_name != "feature/a"));
    let pb = pending.iter().find(|p| p.short_name == "feature/b").expect("b 목록에");
    assert_eq!(pb.sha, b_new, "force-push 후의 새 tip 이 진실이다");
    assert!(pending.iter().any(|p| p.short_name == "feature/c" && p.sha == c_tip));

    // expected_sha 가드: 관리자가 화면에서 검토한 것이 옛 tip 이라면 거부.
    let err = checked(&chk, "start_merge(b, 낡은 sha)", &[], || {
        start_merge(&mgr_t, "origin/feature/b", "main", "origin", Some(b_old.as_str()))
    })
    .expect_err("낡은 검토 기준의 병합은 막혀야 한다");
    assert!(err.to_string().contains("새 push가 있었습니다"), "{err}");
    assert!(!merge_in_progress(&mgr_t).unwrap(), "가드 거부는 상태를 남기지 않는다");

    // 새 tip 으로 검토하고 병합하면 정상 진행.
    let out = checked(&chk, "start_merge(b, 새 sha)", &[], || {
        start_merge(&mgr_t, "origin/feature/b", "main", "origin", Some(b_new.as_str()))
    })
    .unwrap();
    assert!(out.ok && !out.conflicted);
    assert!(checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap().ok);
    ledger.record(bare.path());

    // 최종 장부: b_old 를 제외한 모든 push 커밋이 무결하다.
    let missing = ledger.missing(bare.path());
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].0, b_old);
    assert!(is_ancestor(bare.path(), &a_tip, "refs/heads/main"));
    assert!(is_ancestor(bare.path(), &b_new, "refs/heads/main"));
}

/// 앱은 스스로 절대 force-push 하지 않는다 — src/ 전체에서 push 를 다루는
/// 코드 줄에 강제 옵션(--force, "-f", --force-with-lease, "+refs)이 없음을
/// grep 으로 증명한다. (시나리오 5 의 force-push 는 팀원이 터미널에서 한
/// 앱 밖 행동이었다.)
#[test]
fn app_code_never_force_pushes() {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
    assert!(!files.is_empty());
    let mut push_lines = 0usize;
    for f in &files {
        let body = fs::read_to_string(f).unwrap();
        for (i, line) in body.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if !code.contains("push") {
                continue;
            }
            push_lines += 1;
            for bad in ["--force", "force-with-lease", "\"-f\"", "\"+refs"] {
                assert!(
                    !code.contains(bad),
                    "{}:{} push 코드 경로에 강제 옵션({bad})이 있다: {line}",
                    f.display(),
                    i + 1
                );
            }
        }
    }
    assert!(push_lines > 0, "push 를 다루는 코드가 하나도 안 잡혔다 — 검사가 헛돌았다");
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 6 — delete_remote_branch 적대적 상황
// ═════════════════════════════════════════════════════════════════════════════

/// FIXME(MLOSS-1) [데이터손실]: 팀원이 방금 그 브랜치에 새 커밋을 push 한
/// 직후의 삭제 — 가드는 "삭제 직전에 다시 확인한다"(src/git/merge.rs:358)고
/// 하지만, 실제 검사(merge.rs:384-394)는 관리자의 **로컬 원격 트래킹 ref**
/// (마지막 fetch 시점)를 본다. fetch 이후 도착한 push 는 보이지 않으므로
/// 가드가 통과하고, `push origin --delete` 가 성공해 팀원의 새 커밋이
/// origin 의 어떤 ref 에서도 도달 불가능해진다 — 불변식 (A) 위반.
///
/// 기대: 거부 ("아직 main 에 없는 커밋이 있습니다").
/// 실제: 삭제 성공, 커밋 고아화. 이 테스트는 현재 동작을 고정하고 장부로
/// 유실을 증명한다. 최소 수정: ancestor 검사 전에 해당 브랜치를 fetch
/// 하거나 `git ls-remote <remote> <branch>` 의 실제 tip 과 대조할 것 —
/// 아래 후반부가 "refs 가 신선하면 같은 가드가 거부한다"를 보여 주므로
/// fetch 한 줄이면 충분하다는 증거다.
#[test]
fn s6a_delete_remote_branch_refetches_and_refuses_fresh_push() {
    let (bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());
    let chk = WtChecker::new(mgr.path());
    let mut ledger = OriginLedger::new();
    ledger.record(bare.path());

    // ── 1부: 낡은 트래킹 ref 로 삭제 → 유실 (현재 동작) ──
    let member = person(&url, "seoyeon");
    branch_with_commit(member.path(), "feature/x", "x.txt", "x-v1\n", "feat x v1");
    git(member.path(), &["push", "-q", "origin", "feature/x"]);
    ledger.record(bare.path());

    checked(&chk, "fetch", &[], || fetch_target(&mgr_t, "origin")).unwrap();
    let out = checked(&chk, "start_merge(x)", &[], || {
        start_merge(&mgr_t, "origin/feature/x", "main", "origin", None)
    })
    .unwrap();
    assert!(out.ok);
    assert!(checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap().ok);
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());

    // 팀원이 feature/x 에 새 커밋을 push 한다 — 관리자는 이후 fetch 안 함.
    seed_commit(member.path(), "x.txt", "x-v2 (아직 병합 안 됨)\n", "feat x v2");
    git(member.path(), &["push", "-q", "origin", "feature/x"]);
    let x2 = head_sha(member.path(), "feature/x");
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());

    // 관리자(낡은 refs)가 "병합 끝난" 브랜치를 정리하려 한다 — 회귀 방지
    // (MLOSS-1): 삭제 직전 fetch 로 원격의 실제 tip(x2)을 확인해 거부한다.
    let err = checked(&chk, "delete_remote_branch(낡은 refs)", &[], || {
        delete_remote_branch(&mgr_t, "origin", "main", "feature/x")
    })
    .expect_err("낡은 refs 여도 방금 push 된 커밋을 보고 거부해야 한다");
    assert!(err.to_string().contains("없는 커밋"), "{err}");
    assert!(bare_branch_exists(bare.path(), "feature/x"), "원격 브랜치는 살아남는다");

    // 장부: 아무것도 잃지 않았다 — x2 는 여전히 origin 에서 도달 가능하다.
    ledger.assert_origin_intact(bare.path());
    assert!(OriginLedger::reachable_now(bare.path()).contains(&x2));
    assert!(
        !is_ancestor(bare.path(), &x2, "refs/heads/main"),
        "x2 는 아직 main 에 없다 — 그래서 삭제가 거부된 것이 맞다"
    );

    // ── 2부: refs 가 신선하면 같은 가드가 올바르게 거부한다 ──
    // (= "삭제 전에 fetch 한 줄" 이 최소 수정이라는 증거)
    git(member.path(), &["checkout", "-q", "main"]); // feature/x(고아가 된 x2)에서가 아니라 main 에서 분기
    branch_with_commit(member.path(), "feature/y", "y.txt", "y-v1\n", "feat y v1");
    git(member.path(), &["push", "-q", "origin", "feature/y"]);
    ledger.record(bare.path());
    checked(&chk, "fetch", &[], || fetch_target(&mgr_t, "origin")).unwrap();
    let out = checked(&chk, "start_merge(y)", &[], || {
        start_merge(&mgr_t, "origin/feature/y", "main", "origin", None)
    })
    .unwrap();
    assert!(out.ok);
    assert!(checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap().ok);
    ledger.record(bare.path());

    seed_commit(member.path(), "y.txt", "y-v2\n", "feat y v2");
    git(member.path(), &["push", "-q", "origin", "feature/y"]);
    let y2 = head_sha(member.path(), "feature/y");
    ledger.record(bare.path());

    checked(&chk, "fetch(신선)", &[], || fetch_target(&mgr_t, "origin")).unwrap();
    let err = checked(&chk, "delete_remote_branch(신선 refs)", &[], || {
        delete_remote_branch(&mgr_t, "origin", "main", "feature/y")
    })
    .expect_err("신선한 refs 에서는 가드가 거부한다");
    assert!(err.to_string().contains("없는 커밋"), "{err}");
    assert!(bare_branch_exists(bare.path(), "feature/y"), "y 는 살아남는다");
    assert!(OriginLedger::reachable_now(bare.path()).contains(&y2));

    // 최종 장부: 이 시나리오 전체에서 유실은 0 이다.
    ledger.assert_origin_intact(bare.path());
    let _ = x2;
}

/// 정당한 삭제 뒤의 세상: 낡은 clone 을 가진 팀원이 지워진 브랜치 위에서
/// sync/병합을 해도 아무것도 깨지지 않고 아무것도 잃지 않는다. 삭제는
/// main 도달 가능 커밋을 하나도 지우지 않았다. [정상동작확인]
#[test]
fn s6b_stale_clone_after_legit_delete_breaks_nothing() {
    let (bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());
    let chk = WtChecker::new(mgr.path());
    let mut ledger = OriginLedger::new();
    ledger.record(bare.path());

    let member = person(&url, "junho");
    branch_with_commit(member.path(), "feature/z", "z.txt", "z\n", "feat z");
    git(member.path(), &["push", "-q", "origin", "feature/z"]);
    let z_tip = head_sha(member.path(), "feature/z");
    ledger.record(bare.path());

    checked(&chk, "fetch", &[], || fetch_target(&mgr_t, "origin")).unwrap();
    let out = checked(&chk, "start_merge(z)", &[], || {
        start_merge(&mgr_t, "origin/feature/z", "main", "origin", Some(z_tip.as_str()))
    })
    .unwrap();
    assert!(out.ok);
    assert!(checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap().ok);
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());

    // 팀원은 여전히 feature/z 위에 있다 (낡은 clone — 삭제를 모른다).
    // 정당한 삭제: refs 신선, 완전 병합.
    checked(&chk, "delete_remote_branch(z)", &[], || {
        delete_remote_branch(&mgr_t, "origin", "main", "feature/z")
    })
    .unwrap();
    ledger.assert_deleted_branch_absorbed(bare.path(), "feature/z", "main");
    ledger.assert_origin_intact(bare.path());

    // 낡은 clone 의 팀원: 지워진 브랜치 위에서 sync — 깨지지도 잃지도 않는다.
    let member_t = tgt(member.path());
    let sync = sync_to_base(&member_t, "main", "origin").unwrap();
    assert!(!sync.conflicted, "{}", sync.message);
    assert!(is_ancestor(member.path(), &z_tip, "HEAD"), "자기 커밋은 그대로 자기 브랜치에");
    assert!(is_ancestor(member.path(), &z_tip, "origin/main"), "그리고 origin/main 에도 있다");
    let heads = git(member.path(), &["for-each-ref", "refs/heads", "--format=%(refname:short)"]);
    assert!(heads.contains("feature/z"), "로컬 브랜치는 원격 삭제와 무관하게 남는다");

    // 지워진 트래킹 ref 로 다시 병합을 시도해도 (관리자 쪽) 한국어 안내로
    // 안전하게 실패할 뿐 아무것도 잃지 않는다.
    let err = checked(&chk, "start_merge(지워진 z)", &[], || {
        start_merge(&mgr_t, "origin/feature/z", "main", "origin", None)
    })
    .expect_err("지워진 ref 병합은 실패");
    assert!(err.to_string().contains("찾을 수 없습니다"), "{err}");
    assert!(!merge_in_progress(&mgr_t).unwrap());
    ledger.assert_origin_intact(bare.path());
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 7 — "재시작" 복원력: 모든 중간 상태는 git 만으로 재구성된다
// ═════════════════════════════════════════════════════════════════════════════

/// 병합 센터의 세 중간 상태(충돌 중 / 전부 해결·커밋 전 / 커밋·푸시 전)에서
/// 앱 재시작을 시뮬레이션한다 — 앱은 메모리 상태를 갖지 않으므로 재시작
/// 직후의 화면은 문서화된 상태 조회(merge_state, remaining_conflicts,
/// list_backups, base_unpushed_count, list_pending_branches)를 처음부터 다시
/// 부르는 것과 같다. 각 상태에서 조회가 진실을 말하고, 그 상태의 "다음
/// 행동"이 성공해야 한다. [정상동작확인]
#[test]
fn s7_restart_reconstructs_truth_from_git_alone() {
    let _guard = backup_guard();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("GC_BACKUP_DIR", tmp.path().join("backups"));

    let (bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());
    let chk = WtChecker::new(mgr.path());
    let mut ledger = OriginLedger::new();
    ledger.record(bare.path());

    let member = person(&url, "haneul");
    branch_with_commit(member.path(), "feature/r", "app.txt", "alpha-r\nbeta\ngamma\n", "feat r 1");
    seed_commit(member.path(), "r.txt", "r\n", "feat r 2");
    git(member.path(), &["push", "-q", "origin", "feature/r"]);
    let r_tip = head_sha(member.path(), "feature/r");
    ledger.record(bare.path());

    let other = person(&url, "jisu");
    branch_with_commit(other.path(), "feature/s", "s.txt", "s\n", "feat s");
    git(other.path(), &["push", "-q", "origin", "feature/s"]);
    ledger.record(bare.path());

    seed_commit(mgr.path(), "app.txt", "alpha-main\nbeta\ngamma\n", "main edit");
    assert!(checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap().ok);
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());

    let out = checked(&chk, "start_merge(r)", &[], || {
        start_merge(&mgr_t, "origin/feature/r", "main", "origin", Some(r_tip.as_str()))
    })
    .unwrap();
    assert!(out.conflicted);

    // ── 재시작 #1: 충돌 해결 도중 ──
    // 상태 조회가 전부 git 에서 진실을 재구성한다.
    assert!(merge_in_progress(&mgr_t).unwrap());
    assert_eq!(remaining_conflicts(&mgr_t).unwrap(), vec!["app.txt"]);
    assert!(list_backups(&mgr_t).unwrap().is_empty());
    assert_eq!(base_unpushed_count(&mgr_t, "origin", "main").unwrap(), 0);
    let pending = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    let r = pending.iter().find(|p| p.short_name == "feature/r").expect("아직 병합 전");
    assert!(!r.merged_locally);
    // 재시작 직후 실수로 다른 병합을 눌러도 진행 중인 병합이 지켜진다.
    let err = checked(&chk, "start_merge(s, 병합 중)", &[], || {
        start_merge(&mgr_t, "origin/feature/s", "main", "origin", None)
    })
    .expect_err("병합 중에는 새 병합 거부");
    assert!(err.to_string().contains("이미 진행 중인 병합"), "{err}");
    assert!(merge_in_progress(&mgr_t).unwrap(), "첫 병합은 살아 있다");
    // 이 상태의 다음 행동: 해결.
    let rem = checked(&chk, "resolve(app.txt)", &[], || {
        resolve_conflict(
            &mgr_t,
            "app.txt",
            &Resolution::Manual { content: "alpha-resolved\nbeta\ngamma\n".into() },
        )
    })
    .unwrap();
    assert!(rem.is_empty());

    // ── 재시작 #2: 전부 해결, 커밋 전 ──
    assert!(merge_in_progress(&mgr_t).unwrap(), "커밋 전이므로 병합은 진행 중");
    assert!(remaining_conflicts(&mgr_t).unwrap().is_empty(), "남은 충돌 없음 — '병합 완료' 대기");
    assert_eq!(base_unpushed_count(&mgr_t, "origin", "main").unwrap(), 0);
    // 다음 행동: 병합 완료.
    let done = checked(&chk, "complete_merge(r)", &[], || complete_merge(&mgr_t, None)).unwrap();
    assert!(done.ok, "{}", done.message);

    // ── 재시작 #3: 커밋됨, 푸시 전 ──
    assert!(!merge_in_progress(&mgr_t).unwrap());
    assert_eq!(
        base_unpushed_count(&mgr_t, "origin", "main").unwrap(),
        3,
        "팀원 커밋 2 + 병합 커밋 1"
    );
    let pending = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    let r = pending.iter().find(|p| p.short_name == "feature/r").expect("원격 main 에는 아직 없다");
    assert!(r.merged_locally, "'다시 병합' 이 아니라 '푸시 대기' 로 재구성돼야 한다");
    // 다음 행동: 푸시.
    assert!(checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap().ok);
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());
    assert_eq!(base_unpushed_count(&mgr_t, "origin", "main").unwrap(), 0);
    let pending = list_pending_branches(&mgr_t, "origin", "main").unwrap();
    assert!(pending.iter().all(|p| p.short_name != "feature/r"));
    assert!(is_ancestor(bare.path(), &r_tip, "refs/heads/main"));
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 8 — 백업 수명주기
// ═════════════════════════════════════════════════════════════════════════════

/// 자동 해결 두 번 = 백업 두 개. 더 새 해결 뒤에 더 오래된 백업을 복원해도
/// 복원 대상 파일 외에는 아무것도 바뀌지 않고, 목록 정렬(최신 먼저)이
/// 온전하다. [정상동작확인]
#[test]
fn s8_backup_lifecycle_restore_older_after_newer() {
    let _guard = backup_guard();
    let tmp = TempDir::new().unwrap();
    std::env::set_var("GC_BACKUP_DIR", tmp.path().join("backups"));

    let (bare, url, mgr) = team_origin();
    let mgr_t = tgt(mgr.path());
    let chk = WtChecker::new(mgr.path());
    let mut ledger = OriginLedger::new();
    ledger.record(bare.path());

    let rule_based = AutoResolveOptions {
        binary_strategy: SideChoice::Theirs,
        text_fallback: Some(SideChoice::Theirs),
    };
    let ai_off = |_d: &ConflictDetail| -> Result<String, AppError> {
        Err(AppError::Config("AI 꺼짐".into()))
    };

    // 두 팀원 모두 초기 main 에서 clone.
    let m1 = person(&url, "doyun");
    let m2 = person(&url, "seoyeon");

    // ── 병합 1: app.txt 충돌 → 자동 해결(백업 1) ──
    branch_with_commit(m1.path(), "feature/one", "app.txt", "alpha-one\nbeta\ngamma\n", "feat one");
    git(m1.path(), &["push", "-q", "origin", "feature/one"]);
    ledger.record(bare.path());
    seed_commit(mgr.path(), "app.txt", "alpha-m1\nbeta\ngamma\n", "main edit 1");
    assert!(checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap().ok);
    ledger.record(bare.path());

    let out = checked(&chk, "start_merge(one)", &[], || {
        start_merge(&mgr_t, "origin/feature/one", "main", "origin", None)
    })
    .unwrap();
    assert!(out.conflicted);
    let pre1 = fs::read(mgr.path().join("app.txt")).unwrap(); // 해결 전 마커 본문 1
    let report1 = checked(&chk, "auto_resolve(1)", &[], || {
        auto_resolve_merge(&mgr_t, &rule_based, ai_off)
    })
    .unwrap();
    assert!(report1.committed, "{}", report1.message);
    let backup1 = report1.backup_id.expect("백업 1");
    assert!(checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap().ok);
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());

    // id 는 밀리초 접두라 정렬 가능 — 다음 백업이 뒤에 서게 잠깐 쉰다.
    std::thread::sleep(std::time::Duration::from_millis(5));

    // ── 병합 2: notes.txt 충돌 → 자동 해결(백업 2) ──
    branch_with_commit(m2.path(), "feature/two", "notes.txt", "n1-two\nn2\n", "feat two");
    git(m2.path(), &["push", "-q", "origin", "feature/two"]);
    ledger.record(bare.path());
    seed_commit(mgr.path(), "notes.txt", "n1-m2\nn2\n", "main edit 2");
    assert!(checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap().ok);
    ledger.record(bare.path());

    let out = checked(&chk, "start_merge(two)", &[], || {
        start_merge(&mgr_t, "origin/feature/two", "main", "origin", None)
    })
    .unwrap();
    assert!(out.conflicted);
    assert_eq!(out.conflicted_files, vec!["notes.txt"]);
    let report2 = checked(&chk, "auto_resolve(2)", &[], || {
        auto_resolve_merge(&mgr_t, &rule_based, ai_off)
    })
    .unwrap();
    assert!(report2.committed, "{}", report2.message);
    let backup2 = report2.backup_id.expect("백업 2");
    assert!(checked(&chk, "push(main)", &[], || push(&mgr_t, Some("main"), None)).unwrap().ok);
    ledger.record(bare.path());
    ledger.assert_origin_intact(bare.path());

    // 목록: 최신 먼저, 파일 목록 정확, 생성 시각 파싱 가능.
    let backups = list_backups(&mgr_t).unwrap();
    assert_eq!(backups.len(), 2);
    assert_eq!(backups[0].id, backup2, "최신(병합 2)이 앞");
    assert_eq!(backups[1].id, backup1);
    assert_eq!(backups[0].files, vec!["notes.txt"]);
    assert_eq!(backups[1].files, vec!["app.txt"]);
    assert!(backups[0].created_at >= backups[1].created_at);
    assert!(
        chrono_parse_ok(&backups[0].created_at) && chrono_parse_ok(&backups[1].created_at),
        "created_at 은 RFC3339 여야 한다: {:?} / {:?}",
        backups[0].created_at,
        backups[1].created_at
    );

    // 더 새 해결(병합 2) 뒤에 더 오래된 백업(병합 1)을 복원한다.
    let before = chk.worktree_files();
    let n = checked(&chk, "restore_backup(older)", &[], || restore_backup(&mgr_t, &backup1)).unwrap();
    assert_eq!(n, 1);
    assert_eq!(
        fs::read(mgr.path().join("app.txt")).unwrap(),
        pre1,
        "복원은 병합 1 의 해결 전 본문을 바이트 단위로 되돌린다"
    );
    let after = chk.worktree_files();
    for (path, bytes) in &after {
        if path == "app.txt" {
            continue;
        }
        assert_eq!(before.get(path), Some(bytes), "{path} 는 변하면 안 된다");
    }
    assert_eq!(before.len(), after.len(), "파일이 생기거나 사라지면 안 된다");

    // 커밋된 히스토리·origin 은 복원의 영향을 받지 않는다.
    assert!(!merge_in_progress(&mgr_t).unwrap());
    ledger.assert_origin_intact(bare.path());
}

fn chrono_parse_ok(s: &str) -> bool {
    // RFC3339 형태(연-월-일T…)인지 가볍게 확인 — list_backups 는 파싱 실패 시
    // id 원문을 그대로 돌려주므로, 그 폴백이 아닌지 본다.
    s.len() >= 19 && s.as_bytes()[4] == b'-' && s.as_bytes()[10] == b'T'
}
