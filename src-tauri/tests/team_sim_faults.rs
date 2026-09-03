//! FAULT INJECTION — 험한 실전 장애 시뮬레이션.
//!
//! 요구사항: **어떤 장애(크래시/원격 소실/락 경합/반쯤 쓰인 파일/백업 불가/
//! 스풀 오염)에서도 작업이 증발하면 안 되고, 장애 후 앱은 디스크만으로
//! 진실한 상태를 재구성해야 한다.** 앱 동작은 전부 `git_companion::` 공개
//! API, raw git/fs 는 셋업·장애 주입·검증 전용이다. 변이 연산은 전부
//! 축약형 NO-LOSS 검사기(`checked`, tests/team_sim_noloss_member.rs 의
//! 검사기를 압축 이식)로 감싼다.
//!
//! "크래시"의 재현: 앱 백엔드는 시스템 git 을 부르는 무상태 래퍼라 진실은
//! 전부 디스크(.git + 워크트리)에 있다. 각 단계 직후 클론 디렉토리 전체를
//! `cp -a` 로 새 위치에 복제("여기서 기계가 죽었다; 사용자가 앱을 다시 연다")
//! 하고, 그 사본만으로 상태-읽기 API 가 진실을 말하는지 검증한 뒤 사본에서
//! 흐름을 끝까지 마저 진행해 origin/main 도달성으로 무유실을 증명한다.
//!
//! ── 발견 사항 (본문 FAULT-n 주석과 대응; 1~6 은 고쳐져 새 동작을 회귀-방지로 고정) ──
//!
//! [고침확인/FAULT-6] index.lock 경합 중 `resolve_conflict(Manual)` 은
//!   staging(add) 실패 시 워크트리를 호출 전 바이트로 되돌린 뒤 Err 를 준다
//!   (merge.rs — 쓰기 전 원본을 읽어 두고 add 실패 시 복원). "실패 = 상태
//!   불변": 파일은 충돌 마커 본문 그대로, 스테이지(:1/:2/:3)도 그대로.
//!   락 해제 후 같은 호출이 완주한다. → f3b_index_lock_storm_mid_merge
//!
//! [고침확인/FAULT-1] 원격 소실 시 `fetch_target` 은 friendly_git_error 를
//!   거쳐(fetch.rs:24) "원격(origin)에 접근할 수 없습니다 …" 한국어 안내를
//!   준다 — 영어 원문 노출 없음. → f2_origin_vanishes_and_returns
//!
//! [고침확인/FAULT-2] 원격 폴더가 사라진 push 실패도 같은 "원격(origin)에
//!   접근할 수 없습니다" 분기로 간다 (ops.rs:698-703) — 미등록·접근불가 두
//!   경우를 한 문구로 덮고 `git remote add` 힌트 줄을 유지한다.
//!   → f2_origin_vanishes_and_returns
//!
//! [고침확인/FAULT-3] 쓰기 불가(pre-receive 거부/읽기전용) origin 에 대한
//!   push 실패는 "원격 서버가 푸시를 거부했습니다(서버 정책·권한·읽기
//!   전용일 수 있습니다). 원격 관리자에게 확인하세요." (ops.rs:728-731) —
//!   풀 해도 해결되지 않는 '풀' 오안내가 사라졌다.
//!   → f2b_origin_read_only_push_fails_fetch_still_works
//!
//! [고침확인/FAULT-4] index.lock(그리고 "*.lock … File exists", "cannot
//!   lock ref")은 friendly_git_error 가 "다른 git 작업이 진행 중입니다(잠금
//!   파일)…" 로 번역하고(ops.rs:689-695), add·stash·resolve_conflict·
//!   abort_merge·start_merge(checkout) 경로 전부가 이 헬퍼를 쓴다 — 한국어
//!   접두사 뒤 영어 원문 노출 없음. 남은 한계: git 자체가 `stash push` 의
//!   락 실패에서 아무 출력도 주지 않아(2.43 관측) stash 만은 락을 못 짚고
//!   빈-stderr 폴백 안내가 나간다. → f3_index_lock_storm_clean_repo / f3b
//!
//! [고침확인/FAULT-5] refs/heads/<branch>.lock 경합("cannot lock ref")의
//!   커밋 실패도 explain_commit_failure 가 "다른 git 작업이 진행 중입니다
//!   (.git 잠금 파일)…" 로 설명한다 (ops.rs:141-143). 스테이징된 내용은
//!   보존되어 재시도는 성공한다. → f3_index_lock_storm_clean_repo 4단계
//!
//! [UX격차(사양상 오프라인 설계)/FAULT-7] 원격이 사라져도
//!   list_pending_branches / sync_to_base / start_merge 는 마지막 fetch
//!   시점의 트래킹 ref 로 조용히 "성공"한다 (fetch.rs:4-7 에 문서화된
//!   오프라인 설계). 데이터는 안전하지만 화면 어디에도 "지금 원격을 못
//!   읽었다, 이 목록은 낡았을 수 있다"는 고지가 없다.
//!   → f2_origin_vanishes_and_returns
//!
//! [정상동작확인] 크래시-모든-지점 스냅샷: 관리자 병합 5단계 각각 직후의
//!   디스크 사본만으로 merge_state/remaining/conflict_detail/pending/
//!   base_unpushed/status/backups 전부 진실을 말하고, 그 사본에서 병합을
//!   끝까지 완주해 팀원 커밋이 origin/main 에 실린다. 팀원의 동기화 충돌
//!   중·스태시 복원 충돌 중 크래시도 동일. → f1_*, f1b_*, f1c_*
//!
//! [정상동작확인] 백업 디렉토리 사용 불가 시 auto_resolve_merge 는 **어떤
//!   파일도 건드리기 전에**(auto.rs:158-171, 백업 루프의 `?`) 한국어
//!   메시지("백업 폴더 생성 실패 …")로 실패한다 — 반쯤 해결된 파일 없음,
//!   MERGE_HEAD 그대로. → f5_backup_dir_unavailable_fails_safely
//!
//! [정상동작확인] 스풀(pending_events.jsonl): 깨진 줄은 버리고(재전송 불능
//!   — peer.rs:251 주석의 의도된 동작) 유효 줄만 전송/보존하며, 전송 중
//!   다른 스레드가 덧붙인 꼬리는 그대로 살아남는다 (peer.rs:268-277).
//!   → f6_spool_corruption_and_concurrent_append
//!
//! [정상동작확인] 반쯤 쓰인 수동 해결 파일(크래시로 잘린 내용)은 인덱스의
//!   충돌 스테이지가 남아 있어 resolve_conflict(Manual) 재실행으로 완전
//!   복구된다. 잘린 .gpconfig 는 read_config_effective 가 커밋된 사본으로
//!   폴백한다 (gpconfig.rs:100-147). → f4_*, f4b_*

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;

use tempfile::TempDir;

use git_companion::git::auto::{
    auto_resolve_merge, list_backups, AutoResolveOptions, SideChoice,
};
use git_companion::git::fetch::fetch_target;
use git_companion::git::merge::{base_unpushed_count, delete_remote_branch};
use git_companion::git::ops::{list_stashes, list_status_with_base, StashAction};
use git_companion::git::status::FileChangeKind;
use git_companion::git::{
    abort_merge, add, commit, complete_merge, conflict_detail, create_branch,
    list_pending_branches, merge_in_progress, push, remaining_conflicts, resolve_conflict,
    stash, start_merge, sync_to_base, Resolution, Target,
};

/// 프로세스 전역 env(GC_BACKUP_DIR, XDG_CONFIG_HOME)를 만지거나 읽는 테스트를
/// 직렬화한다 (tests/git_auto_merge.rs / accounts_session.rs 와 같은 패턴).
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ═════════════════════════════════════════════════════════════════════════════
// raw git / fs 헬퍼 (셋업·장애 주입·검증 전용)
// ═════════════════════════════════════════════════════════════════════════════

fn git_try(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .output()
        .expect("git spawn")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_try(dir, args);
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
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, body).unwrap();
}

fn read_file(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_default()
}

fn set_identity(dir: &Path, name: &str, email: &str) {
    git(dir, &["config", "user.name", name]);
    git(dir, &["config", "user.email", email]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "core.editor", "true"]);
}

fn tgt(dir: &Path) -> Target {
    Target::Local(dir.to_path_buf())
}

fn head_sha(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

fn is_ancestor(dir: &Path, a: &str, b: &str) -> bool {
    git_try(dir, &["merge-base", "--is-ancestor", a, b])
        .status
        .success()
}

fn has_hangul(s: &str) -> bool {
    s.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c))
}

/// "기계가 여기서 죽었다" — 디렉토리 전체를 그대로 복제한다. 사본에서 앱을
/// 다시 여는 것은 그 시점 디스크 상태만으로 재기동하는 것과 동치다.
fn crash_copy(src: &Path, dst: &Path) {
    let st = Command::new("cp")
        .arg("-a")
        .arg(src)
        .arg(dst)
        .status()
        .expect("cp spawn");
    assert!(st.success(), "cp -a {} {} 실패", src.display(), dst.display());
}

// ═════════════════════════════════════════════════════════════════════════════
// 축약형 NO-LOSS 검사기 (tests/team_sim_noloss_member.rs 의 검사기 이식)
// ═════════════════════════════════════════════════════════════════════════════

struct Snap {
    files: BTreeMap<String, Vec<u8>>,
    file_blob: BTreeMap<String, String>,
    commits: BTreeSet<String>,
    stashes: Vec<(String, BTreeSet<String>)>,
    unmerged: BTreeMap<String, Vec<String>>,
}

fn walk_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn rec(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let e = entry.unwrap();
            if e.file_name() == ".git" {
                continue;
            }
            let p = e.path();
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                rec(base, &p, out);
            } else if ft.is_file() {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                out.insert(rel, fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    rec(root, root, &mut out);
    out
}

/// `git hash-object --stdin` — `-w` 없이 (ODB 에 써 넣으면 자기 자신을 속인다).
fn hash_bytes(repo: &Path, bytes: &[u8]) -> String {
    let mut child = Command::new("git")
        .args(["hash-object", "--stdin"])
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("git hash-object spawn");
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "hash-object failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// generous=false: refs + HEAD (스냅샷 시점). generous=true: + MERGE_HEAD
/// (사후 검증 — 진행 중 병합은 의미가 정의된 상태다). stash 는 verify 쪽이
/// 별도로 root 에 넣는다.
fn commit_roots(repo: &Path, generous: bool) -> Vec<String> {
    let mut roots: Vec<String> = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )
    .lines()
    .map(|l| l.trim().to_string())
    .filter(|l| !l.is_empty())
    .collect();
    let mut probes = vec!["HEAD"];
    if generous {
        probes.push("MERGE_HEAD");
    }
    for name in probes {
        let out = git_try(repo, &["rev-parse", "-q", "--verify", &format!("{name}^{{commit}}")]);
        if out.status.success() {
            let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !sha.is_empty() {
                roots.push(sha);
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

fn rev_list_commits(repo: &Path, roots: &[String]) -> BTreeSet<String> {
    if roots.is_empty() {
        return BTreeSet::new();
    }
    let mut args: Vec<&str> = vec!["rev-list"];
    args.extend(roots.iter().map(|s| s.as_str()));
    git(repo, &args).lines().map(|l| l.trim().to_string()).collect()
}

fn rev_list_objects(repo: &Path, roots: &[String]) -> BTreeSet<String> {
    if roots.is_empty() {
        return BTreeSet::new();
    }
    let mut args: Vec<&str> = vec!["rev-list", "--objects"];
    args.extend(roots.iter().map(|s| s.as_str()));
    git(repo, &args)
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .map(|s| s.to_string())
        .collect()
}

fn index_blobs(repo: &Path) -> BTreeSet<String> {
    let out = git_try(repo, &["ls-files", "-s"]);
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .map(|s| s.to_string())
        .collect()
}

fn stash_shas(repo: &Path) -> Vec<String> {
    let out = git_try(repo, &["stash", "list", "--format=%H"]);
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn stash_content_blobs(repo: &Path, sha: &str) -> BTreeSet<String> {
    let mut blobs = BTreeSet::new();
    for rev in [sha.to_string(), format!("{sha}^2"), format!("{sha}^3")] {
        let out = git_try(repo, &["ls-tree", "-r", &rev]);
        if !out.status.success() {
            continue;
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut it = line.split_whitespace();
            let _mode = it.next();
            let typ = it.next().unwrap_or("");
            let s = it.next().unwrap_or("");
            if typ == "blob" && !s.is_empty() {
                blobs.insert(s.to_string());
            }
        }
    }
    blobs
}

fn unmerged_stages(repo: &Path) -> BTreeMap<String, Vec<String>> {
    let out = git_try(repo, &["ls-files", "-u"]);
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let (meta, path) = match line.split_once('\t') {
            Some(v) => v,
            None => continue,
        };
        if let Some(sha) = meta.split_whitespace().nth(1) {
            map.entry(path.to_string()).or_default().push(sha.to_string());
        }
    }
    map
}

fn snap(repo: &Path) -> Snap {
    let files = walk_files(repo);
    let file_blob = files
        .iter()
        .map(|(p, b)| (p.clone(), hash_bytes(repo, b)))
        .collect();
    let commits = rev_list_commits(repo, &commit_roots(repo, false));
    let stashes = stash_shas(repo)
        .into_iter()
        .map(|sha| {
            let blobs = stash_content_blobs(repo, &sha);
            (sha, blobs)
        })
        .collect();
    Snap {
        files,
        file_blob,
        commits,
        stashes,
        unmerged: unmerged_stages(repo),
    }
}

fn verify_no_loss(repo: &Path, before: &Snap, label: &str, allow_stash_vanish: bool) {
    let mut roots = commit_roots(repo, true);
    let after_stash = stash_shas(repo);
    roots.extend(after_stash.iter().cloned());
    roots.sort();
    roots.dedup();

    let after_commits = rev_list_commits(repo, &roots);
    let lost: Vec<&String> = before.commits.difference(&after_commits).collect();
    assert!(
        lost.is_empty(),
        "[NO-LOSS 위반] op='{label}': 커밋 {}개 도달 불가: {lost:?}",
        lost.len()
    );

    let mut findable = rev_list_objects(repo, &roots);
    findable.extend(index_blobs(repo));
    let now_files = walk_files(repo);
    for bytes in now_files.values() {
        findable.insert(hash_bytes(repo, bytes));
    }

    for (path, bytes) in &before.files {
        if now_files.get(path) == Some(bytes) {
            continue;
        }
        let sha = &before.file_blob[path];
        if findable.contains(sha) {
            continue;
        }
        // 충돌 중이던 경로의 워크트리 내용은 git 이 만든 마커 합성본이다 —
        // 대신 그 충돌의 각 stage blob(:1/:2/:3)이 계속 찾을 수 있어야 한다.
        if let Some(stage_blobs) = before.unmerged.get(path) {
            let missing: Vec<&String> =
                stage_blobs.iter().filter(|b| !findable.contains(*b)).collect();
            assert!(
                missing.is_empty(),
                "[NO-LOSS 위반] op='{label}': 충돌 중이던 '{path}' 의 stage blob {missing:?} 유실"
            );
            continue;
        }
        panic!(
            "[NO-LOSS 위반] op='{label}' 가 파일 내용을 잃었다: '{path}' (blob {sha}, {}바이트)",
            bytes.len()
        );
    }

    let after_stash_set: BTreeSet<&String> = after_stash.iter().collect();
    for (sha, blobs) in &before.stashes {
        if after_stash_set.contains(sha) || allow_stash_vanish {
            continue;
        }
        for b in blobs {
            assert!(
                findable.contains(b),
                "[NO-LOSS 위반] op='{label}' 가 stash {sha} 의 blob {b} 를 잃었다"
            );
        }
    }
}

fn checked<T>(repo: &Path, label: &str, op: impl FnOnce() -> T) -> T {
    let before = snap(repo);
    let out = op();
    verify_no_loss(repo, &before, label, false);
    out
}

fn checked_stash_drop<T>(repo: &Path, label: &str, op: impl FnOnce() -> T) -> T {
    let before = snap(repo);
    let out = op();
    verify_no_loss(repo, &before, label, true);
    out
}

// ═════════════════════════════════════════════════════════════════════════════
// 팀 리그 — bare origin + 관리자 클론 + 팀원 클론
// ═════════════════════════════════════════════════════════════════════════════

const A_BASE: &str = "a1\ncommon-a\na3\n";
const A_MEMBER: &str = "a1-member\ncommon-a\na3\n";
const A_MANAGER: &str = "a1-manager\ncommon-a\na3\n";
const A_MERGED: &str = "a1-merged\ncommon-a\na3\n";
const B_BASE: &str = "b1\ncommon-b\nb3\n";
const B_MEMBER: &str = "b1-member\ncommon-b\nb3\n";
const B_MANAGER: &str = "b1-manager\ncommon-b\nb3\n";

struct Rig {
    root: TempDir,
    bare: PathBuf,
    /// origin URL — 셋업 중에만 쓰이지만 리그의 정체성 문서로 유지한다.
    _url: String,
    mgr: PathBuf,
    member_sha: String,
}

/// 관리자 병합 충돌 리그: 팀원이 feature/x 에서 a.txt/b.txt 를 고쳐 push,
/// 관리자도 main 에서 같은 줄을 고쳐 push — start_merge 는 두 파일 충돌.
fn manager_conflict_rig() -> Rig {
    let root = TempDir::new().unwrap();
    let bare = root.path().join("origin.git");
    fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    let url = format!("file://{}", bare.display());

    let mgr = root.path().join("manager");
    fs::create_dir(&mgr).unwrap();
    git(&mgr, &["init", "-q", "-b", "main"]);
    set_identity(&mgr, "관리자", "manager@t.com");
    write_file(&mgr, "a.txt", A_BASE);
    write_file(&mgr, "b.txt", B_BASE);
    git(&mgr, &["add", "-A"]);
    git(&mgr, &["commit", "-q", "-m", "init"]);
    git(&mgr, &["remote", "add", "origin", &url]);
    git(&mgr, &["push", "-q", "-u", "origin", "main"]);

    let member = root.path().join("member");
    git(root.path(), &["clone", "-q", &url, "member"]);
    set_identity(&member, "팀원", "member@t.com");
    git(&member, &["checkout", "-q", "-b", "feature/x"]);
    write_file(&member, "a.txt", A_MEMBER);
    write_file(&member, "b.txt", B_MEMBER);
    git(&member, &["add", "-A"]);
    git(&member, &["commit", "-q", "-m", "member work"]);
    git(&member, &["push", "-q", "-u", "origin", "feature/x"]);
    let member_sha = head_sha(&member);

    write_file(&mgr, "a.txt", A_MANAGER);
    write_file(&mgr, "b.txt", B_MANAGER);
    git(&mgr, &["add", "-A"]);
    git(&mgr, &["commit", "-q", "-m", "manager work"]);
    git(&mgr, &["push", "-q", "origin", "main"]);
    git(&mgr, &["fetch", "-q", "--prune", "origin"]);

    Rig {
        root,
        bare,
        _url: url,
        mgr,
        member_sha,
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 1 — 크래시-모든-지점: 관리자의 충돌 병합 5단계.
// ═════════════════════════════════════════════════════════════════════════════

/// 관리자 병합 흐름의 k번째 단계를 앱 API 로 수행한다.
fn run_manager_step(repo: &Path, k: usize) {
    let t = tgt(repo);
    match k {
        1 => {
            let out = checked(repo, "start_merge(origin/feature/x)", || {
                start_merge(&t, "origin/feature/x", "main", "origin", None)
            })
            .unwrap();
            assert!(out.conflicted, "픽스처는 반드시 충돌해야 한다: {}", out.message);
            assert_eq!(out.conflicted_files, vec!["a.txt", "b.txt"]);
        }
        2 => {
            let rem = checked(repo, "resolve a.txt (Manual)", || {
                resolve_conflict(
                    &t,
                    "a.txt",
                    &Resolution::Manual {
                        content: A_MERGED.into(),
                    },
                )
            })
            .unwrap();
            assert_eq!(rem, vec!["b.txt"]);
        }
        3 => {
            let rem = checked(repo, "resolve b.txt (Theirs)", || {
                resolve_conflict(&t, "b.txt", &Resolution::Theirs)
            })
            .unwrap();
            assert!(rem.is_empty(), "남은 충돌 없음이어야: {rem:?}");
        }
        4 => {
            let out = checked(repo, "complete_merge", || complete_merge(&t, None)).unwrap();
            assert!(out.ok, "병합 커밋 실패: {}", out.message);
        }
        5 => {
            let p = checked(repo, "push(main)", || push(&t, None, None)).unwrap();
            assert!(p.ok, "push 실패: {}", p.message);
        }
        _ => unreachable!(),
    }
}

/// 사본("재기동한 앱")에서 상태-읽기 API 전부가 진실을 말하는지 검증한다.
fn assert_truthful_state(copy: &Path, crash_after: usize) {
    let tc = tgt(copy);

    // merge_state (commands::git::merge_state 가 부르는 그 함수).
    let merging = merge_in_progress(&tc).unwrap();
    assert_eq!(
        merging,
        crash_after <= 3,
        "crash@{crash_after}: MERGE_HEAD 존재 여부가 진실과 다르다"
    );

    // remaining_conflicts.
    let rem = remaining_conflicts(&tc).unwrap();
    let expected: Vec<String> = match crash_after {
        1 => vec!["a.txt".into(), "b.txt".into()],
        2 => vec!["b.txt".into()],
        _ => vec![],
    };
    assert_eq!(rem, expected, "crash@{crash_after}: 남은 충돌 목록");

    // conflict_detail — 아직 충돌 중인 파일의 3면 + 워킹 사본.
    if crash_after == 1 {
        let d = conflict_detail(&tc, "a.txt").unwrap();
        assert!(d.ours.contains("a1-manager"), "ours=관리자 쪽: {}", d.ours);
        assert!(d.theirs.contains("a1-member"), "theirs=팀원 쪽: {}", d.theirs);
        assert!(d.working.contains("<<<<<<<"), "워킹 사본에 마커: {}", d.working);
    }
    if crash_after == 2 {
        // a.txt 는 이미 해결된 채 스테이징 — 워크트리에 그대로 남아 있어야.
        assert_eq!(read_file(copy, "a.txt"), A_MERGED);
        let d = conflict_detail(&tc, "b.txt").unwrap();
        assert!(d.ours.contains("b1-manager"));
        assert!(d.theirs.contains("b1-member"));
        assert!(d.working.contains("<<<<<<<"));
    }

    // list_pending_branches — 팀원 브랜치의 병합 상태를 정확히 보고해야.
    let pending = list_pending_branches(&tc, "origin", "main").unwrap();
    let feat = pending.iter().find(|b| b.name == "origin/feature/x");
    match crash_after {
        1..=3 => {
            let f = feat.expect("병합 완료 전에는 feature/x 가 대기 목록에 있어야");
            assert!(!f.merged_locally, "아직 로컬 main 에 병합되지 않았다");
        }
        4 => {
            let f = feat.expect("푸시 전에는 아직 목록에 남는다");
            assert!(
                f.merged_locally,
                "로컬 병합 완료·미푸시 = merged_locally 로 구분돼야 (재병합 방지)"
            );
        }
        _ => assert!(feat.is_none(), "푸시까지 끝나면 대기 목록에서 사라져야: {pending:?}"),
    }

    // base_unpushed_count — "병합은 됐는데 푸시가 안 된" 커밋 수.
    // crash@4: 병합 커밋 1 + (그 병합으로 main 에 처음 실린) 팀원 커밋 1 = 2
    // — rev-list origin/main..main 의 정직한 값이다 (merge.rs:414-434).
    let unpushed = base_unpushed_count(&tc, "origin", "main").unwrap();
    assert_eq!(
        unpushed,
        if crash_after == 4 { 2 } else { 0 },
        "crash@{crash_after}: 미푸시 커밋 수"
    );

    // list_status_with_base — 충돌 파일이 Conflicted 로 잡혀야.
    let st = list_status_with_base(&tc, "main").unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));
    if crash_after <= 2 {
        assert!(
            st.files
                .iter()
                .any(|f| matches!(f.kind, FileChangeKind::Conflicted)),
            "crash@{crash_after}: status 에 충돌 파일이 보여야: {:?}",
            st.files
        );
    }

    // list_backups (GC_BACKUP_DIR) — 자동 해결을 쓴 적 없으니 정직하게 빈 목록.
    let backups = list_backups(&tc).unwrap();
    assert!(backups.is_empty(), "백업이 없어야 정직하다: {backups:?}");

    // 팀원 커밋은 사본의 트래킹 ref 에서 항상 도달 가능해야 한다.
    assert!(
        is_ancestor(copy, &git(copy, &["rev-parse", "origin/feature/x"]).trim().to_string(), "origin/feature/x"),
        "트래킹 ref 자체가 살아 있어야"
    );
}

#[test]
fn f1_manager_crash_at_every_point_reconstructs_and_completes() {
    // list_backups 가 GC_BACKUP_DIR 을 읽으므로 env 직렬화 + 격리 값 설정.
    let _guard = env_guard();
    let env_tmp = TempDir::new().unwrap();
    std::env::set_var("GC_BACKUP_DIR", env_tmp.path().join("backups"));

    for crash_after in 1..=5usize {
        let rig = manager_conflict_rig();

        // 크래시 지점까지 진행 (원본 클론 = 죽을 기계).
        for k in 1..=crash_after {
            run_manager_step(&rig.mgr, k);
        }

        // 기계 사망 → 디스크 사본에서 재기동.
        let resume_root = TempDir::new().unwrap();
        let copy = resume_root.path().join("resumed");
        crash_copy(&rig.mgr, &copy);

        // 사본만으로 진실한 상태 재구성.
        assert_truthful_state(&copy, crash_after);

        // 사본에서 흐름 완주.
        for k in crash_after + 1..=5 {
            run_manager_step(&copy, k);
        }

        // 무유실: 팀원 커밋과 병합 결과가 origin/main 에 실렸다.
        assert!(
            is_ancestor(&rig.bare, &rig.member_sha, "main"),
            "crash@{crash_after}: 팀원 커밋이 origin/main 에서 도달 불가"
        );
        assert_eq!(
            git(&rig.bare, &["show", "main:a.txt"]),
            A_MERGED,
            "crash@{crash_after}: 수동 해결 내용이 origin/main 에 실려야"
        );
        assert_eq!(
            git(&rig.bare, &["show", "main:b.txt"]),
            B_MEMBER,
            "crash@{crash_after}: theirs 해결(팀원 내용)이 origin/main 에 실려야"
        );
        drop(rig); // 원본 기계는 그대로 폐기 — 사본 쪽이 진실을 완성했다.
    }
    std::env::remove_var("GC_BACKUP_DIR");
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 1b — 팀원이 동기화 충돌 도중 크래시.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn f1b_member_crash_mid_sync_conflict_recovers_from_disk() {
    let root = TempDir::new().unwrap();
    let bare = root.path().join("origin.git");
    fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    let url = format!("file://{}", bare.display());

    let mgr = root.path().join("manager");
    fs::create_dir(&mgr).unwrap();
    git(&mgr, &["init", "-q", "-b", "main"]);
    set_identity(&mgr, "관리자", "manager@t.com");
    write_file(&mgr, "app.txt", "top\nmid\nbottom\n");
    git(&mgr, &["add", "-A"]);
    git(&mgr, &["commit", "-q", "-m", "init"]);
    git(&mgr, &["remote", "add", "origin", &url]);
    git(&mgr, &["push", "-q", "-u", "origin", "main"]);

    let mem = root.path().join("member");
    git(root.path(), &["clone", "-q", &url, "member"]);
    set_identity(&mem, "팀원", "member@t.com");
    let t = tgt(&mem);

    checked(&mem, "create_branch(feature/m)", || create_branch(&t, "feature/m")).unwrap();
    write_file(&mem, "app.txt", "top\nmid-member\nbottom\n");
    let c = checked(&mem, "commit(member edit)", || {
        commit(&t, "member edit", true)
    })
    .unwrap();
    assert!(c.ok, "{}", c.message);
    let member_sha = c.sha.clone().unwrap();
    let p = checked(&mem, "push(feature/m)", || push(&t, None, None)).unwrap();
    assert!(p.ok, "{}", p.message);

    // 관리자가 main 의 같은 줄을 고쳐 push (raw git — 상대 역할 재현).
    write_file(&mgr, "app.txt", "top\nmid-manager\nbottom\n");
    git(&mgr, &["add", "-A"]);
    git(&mgr, &["commit", "-q", "-m", "manager edit"]);
    git(&mgr, &["push", "-q", "origin", "main"]);

    // 동기화 → 충돌 → 여기서 기계가 죽는다.
    let sr = checked(&mem, "sync_to_base", || sync_to_base(&t, "main", "origin")).unwrap();
    assert!(sr.conflicted && sr.files == vec!["app.txt".to_string()]);

    let resume_root = TempDir::new().unwrap();
    let copy = resume_root.path().join("resumed");
    crash_copy(&mem, &copy);
    let tc = tgt(&copy);

    // 사본만으로 진실 재구성.
    assert!(merge_in_progress(&tc).unwrap(), "동기화 병합이 진행 중이어야");
    assert_eq!(remaining_conflicts(&tc).unwrap(), vec!["app.txt".to_string()]);
    let d = conflict_detail(&tc, "app.txt").unwrap();
    assert!(d.ours.contains("mid-member"), "sync 의 ours=팀원: {}", d.ours);
    assert!(d.theirs.contains("mid-manager"), "theirs=main 쪽: {}", d.theirs);
    assert!(d.working.contains("<<<<<<<"));
    let st = list_status_with_base(&tc, "main").unwrap();
    assert_eq!(st.branch.as_deref(), Some("feature/m"));

    // 사본에서 완주: 해결 → 병합 완료 → 푸시.
    let rem = checked(&copy, "resolve(Manual)", || {
        resolve_conflict(
            &tc,
            "app.txt",
            &Resolution::Manual {
                content: "top\nmid-both\nbottom\n".into(),
            },
        )
    })
    .unwrap();
    assert!(rem.is_empty());
    let done = checked(&copy, "complete_merge", || complete_merge(&tc, None)).unwrap();
    assert!(done.ok, "{}", done.message);
    let p = checked(&copy, "push(동기화 후)", || push(&tc, None, None)).unwrap();
    assert!(p.ok, "{}", p.message);

    // 무유실: 팀원 커밋 + 관리자 커밋 둘 다 origin/feature/m 에서 도달 가능.
    assert!(is_ancestor(&bare, &member_sha, "feature/m"));
    let mgr_sha = head_sha(&mgr);
    assert!(is_ancestor(&bare, &mgr_sha, "feature/m"));
    assert_eq!(
        git(&bare, &["show", "feature/m:app.txt"]),
        "top\nmid-both\nbottom\n"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 1c — 팀원이 스태시 복원 충돌 도중 크래시.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn f1c_member_crash_mid_stash_pop_conflict_recovers_from_disk() {
    let root = TempDir::new().unwrap();
    let bare = root.path().join("origin.git");
    fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    let url = format!("file://{}", bare.display());
    let mem = root.path().join("member");
    fs::create_dir(&mem).unwrap();
    git(&mem, &["init", "-q", "-b", "main"]);
    set_identity(&mem, "팀원", "member@t.com");
    write_file(&mem, "app.txt", "s1\ns2\n");
    git(&mem, &["add", "-A"]);
    git(&mem, &["commit", "-q", "-m", "init"]);
    git(&mem, &["remote", "add", "origin", &url]);
    git(&mem, &["push", "-q", "-u", "origin", "main"]);
    let t = tgt(&mem);

    // 편집 → 스태시 → 같은 줄을 다르게 고쳐 커밋 → 스태시 복원 = 충돌.
    write_file(&mem, "app.txt", "s1-stash\ns2\n");
    checked(&mem, "stash save", || {
        stash(&t, StashAction::Save { message: Some("작업 보관".into()) })
    })
    .unwrap();
    write_file(&mem, "app.txt", "s1-commit\ns2\n");
    let c = checked(&mem, "commit", || commit(&t, "commit edit", true)).unwrap();
    assert!(c.ok, "{}", c.message);
    let err = checked(&mem, "stash pop(충돌)", || stash(&t, StashAction::Pop))
        .expect_err("충돌 pop 은 Err 여야");
    let msg = err.to_string();
    assert!(
        msg.contains("스태시를 복원하다 충돌") && msg.contains("남아 있습니다"),
        "한국어·행동 지침·항목 보존 안내가 있어야: {msg}"
    );

    // 여기서 기계가 죽는다.
    let resume_root = TempDir::new().unwrap();
    let copy = resume_root.path().join("resumed");
    crash_copy(&mem, &copy);
    let tc = tgt(&copy);

    // 사본만으로 진실 재구성: MERGE_HEAD 없는 충돌 + 스태시 항목 보존.
    assert!(!merge_in_progress(&tc).unwrap(), "stash pop 충돌엔 MERGE_HEAD 가 없다");
    assert_eq!(remaining_conflicts(&tc).unwrap(), vec!["app.txt".to_string()]);
    assert_eq!(list_stashes(&tc).unwrap().len(), 1, "실패한 pop 은 항목을 지우지 않는다");
    let d = conflict_detail(&tc, "app.txt").unwrap();
    assert!(d.ours.contains("s1-commit"), "ours=커밋된 쪽: {}", d.ours);
    assert!(d.theirs.contains("s1-stash"), "theirs=스태시 쪽: {}", d.theirs);
    // 미해결 충돌이 남은 동안 동기화는 거부돼야 한다 (남의 충돌이 동기화
    // 충돌로 둔갑하는 것 방지 — sync.rs:28-36).
    let sync_err = sync_to_base(&tc, "main", "origin").expect_err("충돌 잔존 중 sync 거부");
    assert!(sync_err.to_string().contains("해결되지 않은 충돌"), "{sync_err}");

    // 사본에서 완주: 수동 해결 → 커밋 → 스태시는 남아 있고, 명시적 drop.
    let rem = checked(&copy, "resolve(Manual)", || {
        resolve_conflict(
            &tc,
            "app.txt",
            &Resolution::Manual { content: "s1-both\ns2\n".into() },
        )
    })
    .unwrap();
    assert!(rem.is_empty());
    let c = checked(&copy, "commit(해결)", || commit(&tc, "stash 충돌 해결", true)).unwrap();
    assert!(c.ok, "{}", c.message);
    assert_eq!(list_stashes(&tc).unwrap().len(), 1, "해결 후에도 항목은 남는다");
    checked_stash_drop(&copy, "명시적 stash drop", || stash(&tc, StashAction::Drop)).unwrap();
    assert!(list_stashes(&tc).unwrap().is_empty());
    assert_eq!(read_file(&copy, "app.txt"), "s1-both\ns2\n");
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 2 — origin 이 통째로 사라진다 (폴더 이동) → 되돌아온다.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn f2_origin_vanishes_and_returns() {
    let rig = {
        // 충돌 없는 리그: 팀원 브랜치는 새 파일만 추가한다.
        let root = TempDir::new().unwrap();
        let bare = root.path().join("origin.git");
        fs::create_dir(&bare).unwrap();
        git(&bare, &["init", "-q", "--bare", "-b", "main"]);
        let url = format!("file://{}", bare.display());
        let mgr = root.path().join("manager");
        fs::create_dir(&mgr).unwrap();
        git(&mgr, &["init", "-q", "-b", "main"]);
        set_identity(&mgr, "관리자", "manager@t.com");
        write_file(&mgr, "app.txt", "v1\n");
        git(&mgr, &["add", "-A"]);
        git(&mgr, &["commit", "-q", "-m", "init"]);
        git(&mgr, &["remote", "add", "origin", &url]);
        git(&mgr, &["push", "-q", "-u", "origin", "main"]);
        let member = root.path().join("member");
        git(root.path(), &["clone", "-q", &url, "member"]);
        set_identity(&member, "팀원", "member@t.com");
        git(&member, &["checkout", "-q", "-b", "feature/gone"]);
        write_file(&member, "gone.txt", "member work\n");
        git(&member, &["add", "-A"]);
        git(&member, &["commit", "-q", "-m", "member: gone.txt"]);
        git(&member, &["push", "-q", "-u", "origin", "feature/gone"]);
        let member_sha = head_sha(&member);
        git(&mgr, &["fetch", "-q", "--prune", "origin"]);
        Rig { root, bare, _url: url, mgr, member_sha }
    };
    let t = tgt(&rig.mgr);

    // 관리자에게 푸시할 로컬 커밋 하나를 만들어 둔다.
    write_file(&rig.mgr, "app.txt", "v2\n");
    git(&rig.mgr, &["add", "-A"]);
    git(&rig.mgr, &["commit", "-q", "-m", "local work"]);

    // ── origin 증발 ──
    let gone = rig.root.path().join("origin.gone");
    fs::rename(&rig.bare, &gone).unwrap();

    let wt_before = walk_files(&rig.mgr);

    // push: 패닉 없이 실패 + 한국어 메시지. FAULT-2 회귀 방지: 미등록·접근
    // 불가를 한 문구로 덮는 "원격(origin)에 접근할 수 없습니다" 안내에
    // `git remote add` 힌트 줄이 유지된다 (ops.rs:698-703).
    let p = checked(&rig.mgr, "push(원격 소실)", || push(&t, None, None)).unwrap();
    assert!(!p.ok);
    assert!(has_hangul(&p.message), "한국어 메시지여야: {}", p.message);
    assert!(
        p.message.contains("원격(origin)에 접근할 수 없습니다")
            && p.message.contains("git remote add"),
        "FAULT-2 새 동작 — 접근 불가 안내 + remote add 힌트: {}",
        p.message
    );

    // fetch_target: FAULT-1 회귀 방지 — friendly_git_error 를 거쳐(fetch.rs:24)
    // 같은 한국어 안내를 준다. 영어 원문이 다시 새면 여기서 깨진다.
    let fe = checked(&rig.mgr, "fetch(원격 소실)", || fetch_target(&t, "origin"))
        .expect_err("원격이 없으면 fetch 는 Err");
    let femsg = fe.to_string();
    assert!(
        femsg.contains("원격(origin)에 접근할 수 없습니다") && has_hangul(&femsg),
        "FAULT-1 새 동작 — 한국어 안내: {femsg}"
    );
    assert!(
        !femsg.contains("does not appear to be a git repository"),
        "영어 원문 노출은 회귀다: {femsg}"
    );

    // list_pending_branches / sync_to_base: 마지막 fetch 의 트래킹 ref 로
    // 조용히 성공한다 — 의도된 오프라인 설계(fetch.rs:4-7)지만 stale 고지가
    // 없다. FIXME(FAULT-7) 현재 동작 고정.
    let pending = checked(&rig.mgr, "pending(원격 소실)", || {
        list_pending_branches(&t, "origin", "main")
    })
    .unwrap();
    assert!(
        pending.iter().any(|b| b.name == "origin/feature/gone"),
        "오프라인에서도 stale 목록으로 동작: {pending:?}"
    );
    let sr = checked(&rig.mgr, "sync(원격 소실)", || sync_to_base(&t, "main", "origin")).unwrap();
    assert!(!sr.conflicted, "stale 기준 up-to-date: {}", sr.message);

    // delete_remote_branch: 병합 전 브랜치라 가드가 한국어로 거부 — 삭제 없음.
    let de = checked(&rig.mgr, "delete(원격 소실)", || {
        delete_remote_branch(&t, "origin", "main", "feature/gone")
    })
    .expect_err("병합 전 브랜치 삭제는 거부돼야");
    let demsg = de.to_string();
    assert!(
        has_hangul(&demsg) && demsg.contains("삭제하지 않았습니다"),
        "한국어·행동 가능한 거부: {demsg}"
    );

    // 어떤 실패도 워크트리를 건드리지 않았다.
    assert_eq!(walk_files(&rig.mgr), wt_before, "원격 소실 실패가 워크트리를 훼손");

    // start_merge 도 오프라인에서 stale ref 로 로컬 병합까지는 성공한다
    // (FAULT-7 의 일부) — 그리고 푸시만 실패한다.
    let out = checked(&rig.mgr, "start_merge(원격 소실)", || {
        start_merge(&t, "origin/feature/gone", "main", "origin", None)
    })
    .unwrap();
    assert!(out.ok && !out.conflicted, "{}", out.message);
    // 미푸시 3개: 소실 전에 만든 "local work" + 병합 커밋 + 팀원 커밋.
    assert_eq!(base_unpushed_count(&t, "origin", "main").unwrap(), 3);
    let p = checked(&rig.mgr, "push(병합 후, 원격 소실)", || push(&t, None, None)).unwrap();
    assert!(!p.ok && has_hangul(&p.message));

    // ── origin 귀환 → 전부 정상 재개 ──
    fs::rename(&gone, &rig.bare).unwrap();
    checked(&rig.mgr, "fetch(복구)", || fetch_target(&t, "origin")).unwrap();
    let p = checked(&rig.mgr, "push(복구)", || push(&t, None, None)).unwrap();
    assert!(p.ok, "복구 후 push 는 성공해야: {}", p.message);
    assert_eq!(base_unpushed_count(&t, "origin", "main").unwrap(), 0);
    assert!(
        is_ancestor(&rig.bare, &rig.member_sha, "main"),
        "팀원 커밋이 origin/main 에 실려야"
    );
    // 병합이 끝난 브랜치는 이제 삭제 가능 — 그리고 커밋은 main 에 남는다.
    checked(&rig.mgr, "delete(복구)", || {
        delete_remote_branch(&t, "origin", "main", "feature/gone")
    })
    .unwrap();
    assert!(
        !git_try(&rig.bare, &["rev-parse", "-q", "--verify", "refs/heads/feature/gone"])
            .status
            .success(),
        "원격 브랜치는 지워졌어야"
    );
    assert!(is_ancestor(&rig.bare, &rig.member_sha, "main"), "삭제 후에도 커밋 보존");
    assert!(
        list_pending_branches(&t, "origin", "main").unwrap().is_empty(),
        "정리 후 대기 목록은 비어야"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 2b — origin 이 읽기 전용이 된다: push 만 실패, fetch 는 산다.
//
// 참고: 이 테스트는 root 로도 결정적으로 돌아야 한다. root 에게 chmod -w 는
// 접근 제어가 되지 않으므로(리눅스에서 root 는 모드 비트를 무시한다), bare 에
// pre-receive 훅(exit 1)을 심어 "쓸 수 없는 origin"을 재현한다 — push 는
// 수신측에서 거부되고 fetch(읽기)는 훅과 무관하게 동작한다는 점에서 파일
// 시스템 읽기전용과 같은 관측 결과를 준다. chmod 도 함께 수행해 둔다.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn f2b_origin_read_only_push_fails_fetch_still_works() {
    let rig = manager_conflict_rig();
    let t = tgt(&rig.mgr);

    // 푸시할 로컬 커밋.
    write_file(&rig.mgr, "notes.txt", "새 메모\n");
    git(&rig.mgr, &["add", "-A"]);
    git(&rig.mgr, &["commit", "-q", "-m", "메모 추가"]);
    let local_sha = head_sha(&rig.mgr);

    // origin 을 "쓰기 불가"로: pre-receive 거부 훅 + (비 root 라면 의미를
    // 갖는) 재귀 chmod a-w.
    let hooks = rig.bare.join("hooks");
    let hook = hooks.join("pre-receive");
    fs::write(&hook, "#!/bin/sh\necho 'read-only filesystem' >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let wt_before = walk_files(&rig.mgr);

    // push: 실패하되 패닉/훼손 없음. FAULT-3 회귀 방지: "remote rejected"/
    // pre-receive 거부는 "원격 서버가 푸시를 거부했습니다(…) 원격 관리자에게
    // 확인하세요." 로 분류된다 (ops.rs:728-731) — 풀 해도 해결되지 않는
    // 상황에 '풀' 처방을 내리던 오안내가 다시 나오면 여기서 깨진다.
    let p = checked(&rig.mgr, "push(읽기전용 origin)", || push(&t, None, None)).unwrap();
    assert!(!p.ok);
    assert!(has_hangul(&p.message), "한국어이긴 해야: {}", p.message);
    assert!(
        p.message.contains("푸시를 거부했습니다") && p.message.contains("원격 관리자"),
        "FAULT-3 새 동작 — 거부 원인·행동 지침: {}",
        p.message
    );
    assert!(
        !p.message.contains("풀"),
        "거부/읽기전용에 '풀' 오안내는 회귀다: {}",
        p.message
    );

    // fetch 는 여전히 동작한다 (읽기는 훅/쓰기권한과 무관).
    checked(&rig.mgr, "fetch(읽기전용 origin)", || fetch_target(&t, "origin")).unwrap();

    // 로컬 상태 무손상: 워크트리 그대로, 커밋 그대로.
    assert_eq!(walk_files(&rig.mgr), wt_before);
    assert_eq!(head_sha(&rig.mgr), local_sha);

    // 권한 복구(훅 제거) → push 재개, origin 에 실린다.
    fs::remove_file(&hook).unwrap();
    let p = checked(&rig.mgr, "push(복구)", || push(&t, None, None)).unwrap();
    assert!(p.ok, "{}", p.message);
    assert!(is_ancestor(&rig.bare, &local_sha, "main"));
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 3 — index.lock 폭풍 (깨끗한 저장소): commit/add/stash/start_merge.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn f3_index_lock_storm_clean_repo() {
    // 충돌 없는 병합 대상 브랜치가 있는 관리자 클론.
    let root = TempDir::new().unwrap();
    let bare = root.path().join("origin.git");
    fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    let url = format!("file://{}", bare.display());
    let mgr = root.path().join("manager");
    fs::create_dir(&mgr).unwrap();
    git(&mgr, &["init", "-q", "-b", "main"]);
    set_identity(&mgr, "관리자", "manager@t.com");
    write_file(&mgr, "a.txt", "본문\n");
    git(&mgr, &["add", "-A"]);
    git(&mgr, &["commit", "-q", "-m", "init"]);
    git(&mgr, &["remote", "add", "origin", &url]);
    git(&mgr, &["push", "-q", "-u", "origin", "main"]);
    git(&mgr, &["checkout", "-q", "-b", "feature/lockme"]);
    write_file(&mgr, "lockme.txt", "브랜치 작업\n");
    git(&mgr, &["add", "-A"]);
    git(&mgr, &["commit", "-q", "-m", "feature work"]);
    git(&mgr, &["push", "-q", "-u", "origin", "feature/lockme"]);
    git(&mgr, &["checkout", "-q", "main"]);
    let t = tgt(&mgr);
    let lock = mgr.join(".git/index.lock");

    // ── 1단계: 깨끗한 트리 + 락 → start_merge 는 실패, 상태 불변 ──
    fs::write(&lock, "").unwrap();
    let head_before = head_sha(&mgr);
    let wt_before = walk_files(&mgr);
    let err = checked(&mgr, "start_merge(index.lock)", || {
        start_merge(&t, "origin/feature/lockme", "main", "origin", None)
    })
    .expect_err("락 중 start_merge 는 Err");
    let msg = err.to_string();
    // FAULT-4 회귀 방지: checkout 의 stderr 도 friendly_git_error 를 거쳐
    // "checkout main 실패: 다른 git 작업이 진행 중입니다(잠금 파일)…" 로
    // 나간다 (merge.rs:524-530) — 영어 원문이 다시 새면 여기서 깨진다.
    assert!(msg.contains("index.lock"), "락이 원인임은 드러나야: {msg}");
    assert!(
        msg.contains("checkout main 실패") && msg.contains("다른 git 작업이 진행 중"),
        "FAULT-4 새 동작 — 한국어 번역: {msg}"
    );
    assert!(!msg.contains("Unable to create"), "영어 원문 노출은 회귀다: {msg}");
    assert!(!merge_in_progress(&t).unwrap(), "MERGE_HEAD 가 생기면 안 된다");
    assert_eq!(head_sha(&mgr), head_before);
    assert_eq!(walk_files(&mgr), wt_before, "실패한 start_merge 가 워크트리를 훼손");

    // ── 2단계: 더러운 트리 + 락 → commit/add/stash 전부 정직한 실패 ──
    write_file(&mgr, "a.txt", "본문\n잠금 중 편집\n");
    let dirty = read_file(&mgr, "a.txt");

    // commit: 유일하게 완전 번역된 경로 (ops.rs:141-143) — 정상동작확인.
    let c = checked(&mgr, "commit(index.lock)", || commit(&t, "잠금 중 커밋", true)).unwrap();
    assert!(!c.ok && c.sha.is_none());
    assert!(
        c.message.contains("다른 git 작업이 진행 중"),
        "index.lock 은 한국어로 설명돼야: {}",
        c.message
    );

    // add: FAULT-4 회귀 방지 — "스테이징 실패: 다른 git 작업이 진행 중…"
    // (ops.rs:85-91, friendly_git_error 통과). 영어 원문 노출 없음.
    let ae = checked(&mgr, "add(index.lock)", || add(&t, &["a.txt".to_string()]))
        .expect_err("락 중 add 는 Err");
    let amsg = ae.to_string();
    assert!(amsg.contains("스테이징 실패") && amsg.contains("index.lock"), "{amsg}");
    assert!(
        amsg.contains("다른 git 작업이 진행 중") && !amsg.contains("Unable to create"),
        "FAULT-4 새 동작 — 영어 원문이 다시 새면 회귀: {amsg}"
    );

    // stash: FAULT-4 — 이 경로도 friendly_git_error 를 거친다 (ops.rs:577-580).
    // 다만 git(2.43) 의 `stash push` 는 index.lock 경합에서 stdout/stderr 둘 다
    // 비운 채 exit 1 만 준다 — 번역기가 받을 원문 자체가 없어 "다른 git 작업"
    // 문구까지는 못 간다(빈 stderr 폴백 안내). 락이 원인임은 위 add/commit
    // 경로가 못박고, 여기서는 한국어 접두사 + 영어 원문 부재만 고정한다.
    let se = checked(&mgr, "stash(index.lock)", || {
        stash(&t, StashAction::Save { message: Some("잠금 중 보관".into()) })
    })
    .expect_err("락 중 stash 는 Err");
    let smsg = se.to_string();
    assert!(smsg.contains("스태시 저장 실패"), "{smsg}");
    assert!(
        !smsg.contains("Unable to create") && !smsg.contains("File exists"),
        "영어 원문 노출은 회귀다: {smsg}"
    );
    assert!(list_stashes(&t).unwrap().is_empty(), "실패한 stash 는 항목을 만들지 않는다");
    assert_eq!(read_file(&mgr, "a.txt"), dirty, "더러운 편집은 그대로 남아야");

    // ── 3단계: 락 해제 → 같은 연산이 전부 성공 ──
    fs::remove_file(&lock).unwrap();
    checked(&mgr, "stash save(해제 후)", || {
        stash(&t, StashAction::Save { message: Some("보관".into()) })
    })
    .unwrap();
    assert_eq!(list_stashes(&t).unwrap().len(), 1);
    checked(&mgr, "stash pop(해제 후)", || stash(&t, StashAction::Pop)).unwrap();
    assert_eq!(read_file(&mgr, "a.txt"), dirty);
    let c = checked(&mgr, "commit(해제 후)", || commit(&t, "잠금 해제 후 커밋", true)).unwrap();
    assert!(c.ok, "{}", c.message);
    let out = checked(&mgr, "start_merge(해제 후)", || {
        start_merge(&t, "origin/feature/lockme", "main", "origin", None)
    })
    .unwrap();
    assert!(out.ok && !out.conflicted, "{}", out.message);

    // ── 4단계: refs/heads/<branch>.lock — 커밋 실패해도 스테이징은 보존 ──
    let ref_lock = mgr.join(".git/refs/heads/main.lock");
    fs::write(&ref_lock, "").unwrap();
    write_file(&mgr, "a.txt", "본문\n잠금 중 편집\nref lock 편집\n");
    let c = checked(&mgr, "commit(ref lock)", || commit(&t, "ref 잠금 커밋", true)).unwrap();
    assert!(!c.ok);
    // FAULT-5 회귀 방지: "cannot lock ref" 도 explain_commit_failure 가 잡아
    // "다른 git 작업이 진행 중입니다(.git 잠금 파일)…" 로 설명한다
    // (ops.rs:141-143) — 완전 영어 원문이 다시 나가면 여기서 깨진다.
    assert!(
        c.message.contains("다른 git 작업이 진행 중") && has_hangul(&c.message),
        "FAULT-5 새 동작 — 한국어 설명: {}",
        c.message
    );
    // 스테이징된 내용은 인덱스에 남아 재시도가 성공한다 — 유실 없음.
    assert!(
        !git(&mgr, &["diff", "--cached", "--name-only"]).trim().is_empty(),
        "실패한 커밋 후에도 스테이징은 남아야"
    );
    fs::remove_file(&ref_lock).unwrap();
    let c = checked(&mgr, "commit(ref lock 해제 후)", || {
        commit(&t, "ref 잠금 해제 후 커밋", false)
    })
    .unwrap();
    assert!(c.ok, "{}", c.message);
    assert!(git(&mgr, &["show", "HEAD:a.txt"]).contains("ref lock 편집"));
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 3b — index.lock 폭풍 (충돌 병합 도중): resolve/abort.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn f3b_index_lock_storm_mid_merge() {
    let rig = manager_conflict_rig();
    let t = tgt(&rig.mgr);
    let out = start_merge(&t, "origin/feature/x", "main", "origin", None).unwrap();
    assert!(out.conflicted);
    let merge_head = git(&rig.mgr, &["rev-parse", "MERGE_HEAD"]).trim().to_string();
    let markers_a = fs::read(rig.mgr.join("a.txt")).unwrap();

    let lock = rig.mgr.join(".git/index.lock");
    fs::write(&lock, "").unwrap();

    // resolve(Ours): 실패 + 워킹 파일/충돌 목록 불변.
    let e = checked(&rig.mgr, "resolve Ours(index.lock)", || {
        resolve_conflict(&t, "a.txt", &Resolution::Ours)
    })
    .expect_err("락 중 resolve 는 Err");
    // FAULT-4 회귀 방지: "ours 해결 실패: 다른 git 작업이 진행 중…"
    // (merge.rs:634-637, friendly_git_error 통과) — 영어 원문 노출 없음.
    assert!(e.to_string().contains("해결 실패") && e.to_string().contains("index.lock"), "{e}");
    assert!(
        e.to_string().contains("다른 git 작업이 진행 중"),
        "FAULT-4 새 동작 — 한국어 번역: {e}"
    );
    assert_eq!(
        remaining_conflicts(&t).unwrap(),
        vec!["a.txt".to_string(), "b.txt".to_string()]
    );
    assert_eq!(fs::read(rig.mgr.join("a.txt")).unwrap(), markers_a, "워킹 파일 불변");

    // FAULT-6 회귀 방지: resolve(Manual) 의 staging(add) 이 실패하면
    // 워크트리를 호출 전 바이트로 되돌린 뒤 Err 를 준다 (merge.rs:664-678)
    // — "실패 = 상태 불변". Err 인데 파일만 새 내용으로 바뀌어 있던 거짓
    // 상태가 다시 생기면 여기서 깨진다.
    let e = checked(&rig.mgr, "resolve Manual(index.lock)", || {
        resolve_conflict(&t, "a.txt", &Resolution::Manual { content: A_MERGED.into() })
    })
    .expect_err("락 중 Manual resolve 도 Err");
    assert!(e.to_string().contains("staging 실패"), "{e}");
    assert!(
        e.to_string().contains("다른 git 작업이 진행 중"),
        "FAULT-4 새 동작 — staging 실패도 한국어 번역: {e}"
    );
    assert_eq!(
        fs::read(rig.mgr.join("a.txt")).unwrap(),
        markers_a,
        "FAULT-6 새 동작: 실패한 Manual resolve 는 워크트리를 호출 전(충돌 마커) 바이트로 되돌린다"
    );
    assert_eq!(
        remaining_conflicts(&t).unwrap(),
        vec!["a.txt".to_string(), "b.txt".to_string()],
        "인덱스는 여전히 충돌 상태 — 재시도로 복구 가능"
    );

    // abort_merge: 실패를 실패라고 말한다 (merge.rs:787-796, MLOSS-2 회귀 방지).
    // FAULT-4 회귀 방지: 이 경로도 "병합 중단 실패: 다른 git 작업…" 한국어다.
    let e = checked(&rig.mgr, "abort(index.lock)", || abort_merge(&t))
        .expect_err("락 중 abort 는 Err");
    assert!(e.to_string().contains("병합 중단 실패"), "{e}");
    assert!(
        e.to_string().contains("다른 git 작업이 진행 중"),
        "FAULT-4 새 동작: {e}"
    );
    assert!(merge_in_progress(&t).unwrap(), "MERGE_HEAD 는 그대로");
    assert_eq!(
        git(&rig.mgr, &["rev-parse", "MERGE_HEAD"]).trim(),
        merge_head,
        "병합 대상도 그대로"
    );

    // 병합 중 start_merge: 락과 무관하게 진행 중 병합 가드가 막는다.
    let e = start_merge(&t, "origin/feature/x", "main", "origin", None)
        .expect_err("병합 중 새 병합은 거부");
    assert!(e.to_string().contains("이미 진행 중인 병합"), "{e}");

    // ── 락 해제 → 같은 연산으로 완주 ──
    fs::remove_file(&lock).unwrap();
    let rem = checked(&rig.mgr, "resolve Manual(해제 후)", || {
        resolve_conflict(&t, "a.txt", &Resolution::Manual { content: A_MERGED.into() })
    })
    .unwrap();
    assert_eq!(rem, vec!["b.txt".to_string()]);
    let rem = checked(&rig.mgr, "resolve Theirs(해제 후)", || {
        resolve_conflict(&t, "b.txt", &Resolution::Theirs)
    })
    .unwrap();
    assert!(rem.is_empty());
    let done = checked(&rig.mgr, "complete(해제 후)", || complete_merge(&t, None)).unwrap();
    assert!(done.ok, "{}", done.message);
    let p = checked(&rig.mgr, "push(해제 후)", || push(&t, None, None)).unwrap();
    assert!(p.ok, "{}", p.message);
    assert!(is_ancestor(&rig.bare, &rig.member_sha, "main"));
    assert_eq!(git(&rig.bare, &["show", "main:a.txt"]), A_MERGED);
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 4 — 반쯤 쓰인 파일: 수동 해결 저장 도중 크래시.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn f4_half_written_manual_resolution_recovers() {
    let rig = manager_conflict_rig();
    let t = tgt(&rig.mgr);
    let out = start_merge(&t, "origin/feature/x", "main", "origin", None).unwrap();
    assert!(out.conflicted);

    // 앱의 Manual 저장(write_file_at_target = fs::write, 원자적이지 않음)이
    // 도중에 죽은 상황: 해결 본문의 앞 7바이트만 워크트리에 남았다.
    let truncated = &A_MERGED[..7];
    fs::write(rig.mgr.join("a.txt"), truncated).unwrap();

    // 재기동한 앱이 보는 진실: 여전히 충돌 중(스테이지는 인덱스에 안전),
    // 워킹 사본은 잘린 그대로 보고된다.
    assert_eq!(
        remaining_conflicts(&t).unwrap(),
        vec!["a.txt".to_string(), "b.txt".to_string()],
        "잘린 워킹 파일이 있어도 인덱스의 충돌 상태가 진실"
    );
    let d = conflict_detail(&t, "a.txt").unwrap();
    assert_eq!(d.working, truncated, "워킹 사본은 있는 그대로 보고");
    assert!(d.ours.contains("a1-manager") && d.theirs.contains("a1-member"),
        "양쪽 원본은 스테이지에 온전");

    // 같은 해결을 다시 실행 → 완전 복구.
    let rem = checked(&rig.mgr, "resolve Manual(재실행)", || {
        resolve_conflict(&t, "a.txt", &Resolution::Manual { content: A_MERGED.into() })
    })
    .unwrap();
    assert_eq!(rem, vec!["b.txt".to_string()]);
    checked(&rig.mgr, "resolve Theirs", || {
        resolve_conflict(&t, "b.txt", &Resolution::Theirs)
    })
    .unwrap();
    let done = checked(&rig.mgr, "complete", || complete_merge(&t, None)).unwrap();
    assert!(done.ok);
    let p = checked(&rig.mgr, "push", || push(&t, None, None)).unwrap();
    assert!(p.ok, "{}", p.message);
    assert_eq!(git(&rig.bare, &["show", "main:a.txt"]), A_MERGED);
    assert!(is_ancestor(&rig.bare, &rig.member_sha, "main"));
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 4b — 잘린 .gpconfig: 커밋된 사본으로 폴백해야 한다.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn f4b_corrupt_gpconfig_falls_back_to_committed_copy() {
    use git_companion::gpconfig::{
        commit_config, is_merge_target, read_config, read_config_effective, save_config,
        GpMember, ProjectConfig,
    };

    let root = TempDir::new().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    set_identity(&repo, "관리자", "manager@t.com");
    write_file(&repo, "README.md", "x\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let t = tgt(&repo);

    // 앱 API 로 팀 설정을 만들고 커밋한다.
    let mut cfg = ProjectConfig::default();
    cfg.default_base_branch = "main".into();
    cfg.members.push(GpMember {
        id: "1".into(),
        name: "리드".into(),
        email: "lead@t.com".into(),
        role: "admin".into(),
    });
    cfg.merge_managers.insert("main".into(), "lead@t.com".into());
    cfg.merge_targets = vec!["main".into()];
    save_config(&t, &cfg).unwrap();
    commit_config(&t).unwrap();

    // 크래시로 반쯤 쓰인(잘린 JSON) 워크트리 사본.
    fs::write(repo.join(".gpconfig"), br#"{"default_base_branch":"ma"#).unwrap();

    // 워크트리 사본은 못 읽는다 — 그러나 effective 는 커밋된 사본으로 폴백.
    assert!(read_config(&t).is_err(), "잘린 JSON 은 파싱 실패여야");
    let (eff, exists) = read_config_effective(&t, "main", "origin").unwrap();
    assert!(exists, "커밋된 사본이 있으니 exists=true");
    assert_eq!(eff.members.len(), 1);
    assert_eq!(eff.members[0].email, "lead@t.com");
    assert_eq!(
        eff.merge_managers.get("main").map(String::as_str),
        Some("lead@t.com")
    );
    assert!(is_merge_target(&eff, exists, "main", "main"));

    // 충돌 마커가 낀 사본(병합 중 크래시)도 동일하게 폴백.
    fs::write(
        repo.join(".gpconfig"),
        b"<<<<<<< HEAD\n{\"default_base_branch\":\"main\"}\n=======\n{}\n>>>>>>> theirs\n",
    )
    .unwrap();
    let (eff2, exists2) = read_config_effective(&t, "main", "origin").unwrap();
    assert!(exists2);
    assert_eq!(eff2.members[0].email, "lead@t.com");
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 5 — 백업 디렉토리 사용 불가: 자동 해결은 아무것도 건드리기 전에
// 안전하게 실패해야 한다 (auto.rs:158-171 — 백업이 해결보다 먼저).
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn f5_backup_dir_unavailable_fails_safely() {
    let _guard = env_guard();
    let rig = manager_conflict_rig();
    let t = tgt(&rig.mgr);
    let out = start_merge(&t, "origin/feature/x", "main", "origin", None).unwrap();
    assert!(out.conflicted);

    let merge_head = git(&rig.mgr, &["rev-parse", "MERGE_HEAD"]).trim().to_string();
    let wt_before = walk_files(&rig.mgr);
    let rem_before = remaining_conflicts(&t).unwrap();

    // GC_BACKUP_DIR 의 조상이 일반 파일 → create_dir_all 이 반드시 실패한다
    // (root 로 실행돼도 결정적 — 권한이 아니라 ENOTDIR).
    let blocker = rig.root.path().join("blocker");
    fs::write(&blocker, "파일이지 폴더가 아님").unwrap();
    std::env::set_var("GC_BACKUP_DIR", blocker.join("backups"));

    let rule_based = AutoResolveOptions {
        binary_strategy: SideChoice::Theirs,
        text_fallback: Some(SideChoice::Theirs),
    };
    let ai_off = |_: &git_companion::git::merge::ConflictDetail| {
        Err(git_companion::error::AppError::Config("AI 꺼짐".into()))
    };

    // 계약 검증: 백업 쓰기 실패 = 즉시 Err, 해결은 시작조차 안 됨.
    let err = checked(&rig.mgr, "auto_resolve(백업 불가)", || {
        auto_resolve_merge(&t, &rule_based, ai_off)
    })
    .expect_err("백업을 못 만들면 해결을 시작하면 안 된다");
    let msg = err.to_string();
    assert!(
        has_hangul(&msg) && msg.contains("백업"),
        "백업이 원인임을 한국어로: {msg}"
    );
    assert!(merge_in_progress(&t).unwrap(), "MERGE_HEAD 는 그대로");
    assert_eq!(
        git(&rig.mgr, &["rev-parse", "MERGE_HEAD"]).trim(),
        merge_head
    );
    assert_eq!(remaining_conflicts(&t).unwrap(), rem_before, "반쯤 해결된 파일 없음");
    assert_eq!(walk_files(&rig.mgr), wt_before, "워크트리 바이트 단위로 무변화");

    // 백업 위치를 복구하면 같은 호출이 끝까지 간다.
    let good = rig.root.path().join("backups-good");
    std::env::set_var("GC_BACKUP_DIR", &good);
    let report = checked(&rig.mgr, "auto_resolve(복구)", || {
        auto_resolve_merge(&t, &rule_based, ai_off)
    })
    .unwrap();
    assert!(report.committed, "{}", report.message);
    assert!(report.backup_id.is_some(), "해결 전 백업이 반드시 존재");
    let backups = list_backups(&t).unwrap();
    assert_eq!(backups.len(), 1);
    assert!(backups[0].files.contains(&"a.txt".to_string()));
    assert!(backups[0].files.contains(&"b.txt".to_string()));
    let p = push(&t, None, None).unwrap();
    assert!(p.ok, "{}", p.message);
    assert!(is_ancestor(&rig.bare, &rig.member_sha, "main"));
    std::env::remove_var("GC_BACKUP_DIR");
}

// ═════════════════════════════════════════════════════════════════════════════
// 시나리오 6 — 스풀 오염 + 전송 중 동시 append (peer::flush_spooled_events).
// ═════════════════════════════════════════════════════════════════════════════

/// 첫 요청 처리 **도중**(읽기 이후, 응답 이전 = flush 가 파일을 이미 읽은
/// 뒤) 스풀에 새 줄을 덧붙이는 응답기 — pre-push hook 이 flush 와 동시에
/// 이벤트를 쓰는 경쟁을 결정적으로 재현한다. wiremock 핸들러는 별도 서버
/// 스레드에서 돌므로 진짜 동시-append 다.
struct AppendDuringFlush {
    spool: PathBuf,
    appended: std::sync::atomic::AtomicBool,
}

impl wiremock::Respond for AppendDuringFlush {
    fn respond(&self, _req: &wiremock::Request) -> wiremock::ResponseTemplate {
        use std::sync::atomic::Ordering;
        if !self.appended.swap(true, Ordering::SeqCst) {
            let line = serde_json::json!({
                "project_id": "p1",
                "event_kind": "branch_push",
                "repo_name": "동시성",
                "payload": "{\"seq\":3}"
            })
            .to_string();
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.spool)
                .unwrap();
            writeln!(f, "{line}").unwrap();
        }
        wiremock::ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({ "id": "evt-ok" }))
    }
}

#[test]
fn f6_spool_corruption_and_concurrent_append() {
    use git_companion::peer::{flush_spooled_events, spool_event, SpooledEvent};

    let _guard = env_guard();
    let home = TempDir::new().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", home.path());
    let spool = git_companion::config_store::config_dir()
        .unwrap()
        .join("pending_events.jsonl");

    let ev = |seq: u32| SpooledEvent {
        project_id: "p1".into(),
        event_kind: "branch_push".into(),
        repo_name: "스풀 저장소".into(),
        payload: format!("{{\"seq\":{seq}}}"),
    };

    // 유효 2건 사이에 깨진 줄들(비 JSON / 타입 불일치 / 잘린 JSON / 빈 줄).
    spool_event(&ev(1)).unwrap();
    {
        let mut f = fs::OpenOptions::new().append(true).open(&spool).unwrap();
        writeln!(f, "this is not json at all").unwrap();
        writeln!(f, "{{\"project_id\": 5}}").unwrap();
        writeln!(f).unwrap();
        write!(f, "{{\"project_id\":\"p1\",\"event_ki\n").unwrap();
    }
    spool_event(&ev(2)).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();

    // ── (a) 서버 죽음(닫힌 포트): 유효 줄은 보존, 깨진 줄은 버려진다 ──
    let dead_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
        // 리스너 drop → 그 포트는 즉시 connection refused.
    };
    let sent = rt
        .block_on(flush_spooled_events(
            &format!("http://127.0.0.1:{dead_port}"),
            "tok",
        ))
        .unwrap();
    assert_eq!(sent, 0, "죽은 서버로는 아무것도 못 보낸다");
    let body = fs::read_to_string(&spool).unwrap();
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "유효 2건만 보존돼야: {body:?}");
    for (i, l) in lines.iter().enumerate() {
        let parsed: SpooledEvent =
            serde_json::from_str(l).expect("보존된 줄은 전부 유효 JSON 이어야");
        assert_eq!(parsed.payload, format!("{{\"seq\":{}}}", i + 1));
    }

    // ── (b) 살아난 서버 + 전송 도중 동시 append: 꼬리는 보존된다 ──
    let server = rt.block_on(wiremock::MockServer::start());
    rt.block_on(
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/events"))
            .respond_with(AppendDuringFlush {
                spool: spool.clone(),
                appended: std::sync::atomic::AtomicBool::new(false),
            })
            .mount(&server),
    );
    let sent = rt
        .block_on(flush_spooled_events(&server.uri(), "tok"))
        .unwrap();
    assert_eq!(sent, 2, "읽기 시점의 유효 2건이 전송돼야");
    assert!(spool.exists(), "전송 중 덧붙은 꼬리가 있으면 파일은 남아야");
    let tail = fs::read_to_string(&spool).unwrap();
    let tail_lines: Vec<&str> = tail.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(tail_lines.len(), 1, "동시 append 1건이 그대로: {tail:?}");
    let parsed: SpooledEvent = serde_json::from_str(tail_lines[0]).unwrap();
    assert_eq!(parsed.payload, "{\"seq\":3}");

    // ── (c) 다음 폴링이 꼬리를 마저 보낸다 → 스풀 소멸 ──
    let sent = rt
        .block_on(flush_spooled_events(&server.uri(), "tok"))
        .unwrap();
    assert_eq!(sent, 1);
    assert!(!spool.exists(), "전부 전송되면 스풀은 지워진다");

    // 서버가 실제로 3건을 받았는지 (유실/중복 없음).
    let reqs = rt.block_on(server.received_requests()).unwrap_or_default();
    assert_eq!(reqs.len(), 3, "정확히 3건 전송돼야");

    std::env::remove_var("XDG_CONFIG_HOME");
    // wiremock 서버는 in-process 라 자식 프로세스가 없다 — 고아 없음.
}
