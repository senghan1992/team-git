//! NO-LOSS 불변식 검사기 + 팀원(member) 시나리오 시뮬레이션.
//!
//! 요구사항: **일반 팀원의 작업은 어떤 예기치 못한 상황에서도 절대 휘발되면
//! 안 된다.** 이 파일은 "팀원이 가진 모든 것"을 앱 호출 전에 스냅샷하고,
//! 호출이 성공하든 실패하든 그 후에 아무것도 사라지지 않았음을 기계적으로
//! 검증하는 불변식 검사기(`checked`)를 정의한 뒤, 팀원 쪽 워크플로우 전체를
//! 그 검사기 아래에서 돌린다. 앱 동작은 전부 `git_companion::git::*` +
//! `Target::Local` (그것이 곧 앱이다). raw git 은 셋업/관리자 역할 전용.
//!
//! ── 불변식 정의 (도달 가능성 규칙의 근거) ─────────────────────────────────
//!
//! 스냅샷은 세 가지를 기록한다:
//!
//! 1. **워크트리 파일** (.git 제외 전체 순회 — tracked + untracked):
//!    상대경로 → 내용 바이트 + blob SHA(`git hash-object --stdin`, `-w` 없이
//!    — ODB에 써 넣으면 검사가 자기 자신을 속인다).
//! 2. **커밋 집합**: refs/heads + refs/remotes + refs/tags + HEAD 에서
//!    `git rev-list` 로 도달 가능한 모든 커밋.
//!    - `--all` 을 쓰지 않는 이유: `--all` 은 refs/stash(=stash@{0})를
//!      포함하지만 stash@{1}… 는 reflog 에만 있어 어차피 못 잡고, 반대로
//!      정상적인 `stash pop` 이 stash 커밋을 지우는 것까지 위반으로 잡는다.
//!      stash 는 "내용 운반체"이므로 3번 규칙으로 따로 검증한다.
//!    - `--reflog` 를 쓰지 않는 이유: reflog 에만 남은 커밋은 gc 대상이고
//!      초보 사용자에게는 사실상 보이지 않는다. 이 검사기는 그것을
//!      손실로 간주한다(엄격한 쪽 선택).
//! 3. **stash 항목**: `git stash list --format=%H` 의 각 SHA 와, 그 항목이
//!    운반하는 blob 집합(`ls-tree -r <sha>`, `<sha>^2`(index), `<sha>^3`
//!    (untracked)).
//!
//! 검증(작업 후):
//!
//! - 스냅샷의 모든 커밋이 여전히 도달 가능해야 한다. 사후 도달 기준(root)은
//!   refs/* + HEAD + MERGE_HEAD + 모든 stash SHA. MERGE_HEAD 는 인정한다 —
//!   진행 중 병합을 완료하면 부모로 흡수되는, 의미가 정의된 상태 파일이다.
//!   ORIG_HEAD 는 인정하지 **않는다** — 다음 merge/reset/pull 이 한 칸짜리
//!   슬롯을 소리 없이 덮어쓰므로 reflog 와 같은 gc-bait 로 취급한다
//!   (`reset --hard` 로 날린 커밋이 ORIG_HEAD 에 걸려 있다고 "안 잃었다"고
//!   말하면 안 된다 — s0 자기 검증이 이를 강제한다).
//! - 스냅샷의 모든 파일 내용이 여전히 **찾을 수 있어야** 한다:
//!   (a) 워크트리 어딘가에 바이트 동일 내용으로 존재하거나,
//!   (b) 그 blob 이 index(충돌 stage 1~3 포함) 또는 위 root 들에서
//!       `rev-list --objects` 로 도달 가능해야 한다.
//!   ODB 에 dangling 으로만 존재하는 것은 **인정하지 않는다** — gc 한 번이면
//!   사라지는 것은 "찾을 수 있다"가 아니다.
//!   예외 한 가지: 스냅샷 시점에 **unmerged(충돌) 상태였던 경로**의 워크트리
//!   내용은 git 이 stage 2/3/base 로부터 생성한 마커 합성본(스크래치)이다.
//!   해결(resolve)이 그것을 대체하는 것은 손실이 아니므로, 합성본 자체 대신
//!   그 충돌의 각 stage blob(팀원 쪽 :2, 상대 쪽 :3, base :1)이 계속 찾을 수
//!   있음을 요구한다. 한계: 사용자가 합성본 안에 손으로 새로 타이핑한 내용은
//!    해결 연산이 대체하는 순간까지만 보호된다 — 양쪽 원본은 항상 보호된다.
//! - 사라진 stash 항목은 그 blob 이 전부 위 기준으로 찾을 수 있어야 한다
//!   (성공한 pop 이 이 경우다). 명시적 drop 만 `checked_stash_drop` 으로
//!   면제된다 — 사용자가 스스로 버린 것은 손실이 아니다.
//!
//! ── "크래시" 논증 (시나리오 7) ────────────────────────────────────────────
//!
//! kill -9 를 프로세스 안에서 흉내낼 수는 없다. 대신 다음 사실로 대신한다:
//! `git_companion::git::{ops,sync,merge,status,fetch}` 의 모든 공개 함수는
//! 시스템 `git` 바이너리를 호출하는 무상태(stateless) 래퍼다 — 백엔드에는
//! 호출 사이에 살아남아야 하는 인메모리 상태가 전혀 없고, 진실은 전부
//! `.git` 디렉토리에 있다. 따라서 "임의의 두 단계 사이에서 죽고 재시작"은
//! "지금 그대로의 저장소에 새 API 호출을 시작"과 동치이며, 이 파일의 모든
//! 테스트는 구조적으로 그렇게(호출마다 독립적으로) 동작한다. 시나리오 7은
//! 충돌 해결 도중 상태를 fresh 호출만으로 완전히 복원할 수 있음을 추가로
//! 못박는다.
//!
//! ── 발견된 문제는 FIXME(LOSS-n)/FIXME(BUG-n)/FIXME(UX-n) 주석과 함께
//!    **현재 동작을 그대로 assert** 한다 (스위트는 green 유지). ──────────

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

use git_companion::git::fetch::fetch_target;
use git_companion::git::merge::{
    complete_merge, conflict_detail, delete_remote_branch, merge_in_progress, remaining_conflicts,
    resolve_conflict, start_merge, Resolution,
};
use git_companion::git::ops::{list_stashes, list_status_with_base, StashAction};
use git_companion::git::{
    add, checkout_branch, commit, create_branch, list_status, pull, push, run_pull_and_merge,
    stash, sync_to_base, write_file_at_target, Target,
};

// ═════════════════════════════════════════════════════════════════════════
// raw git 헬퍼 (셋업/검증 전용 — 앱 동작에는 절대 쓰지 않는다)
// ═════════════════════════════════════════════════════════════════════════

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

/// 바이트 그대로 (CRLF 검증용).
fn git_bytes(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = git_try(dir, args);
    assert!(out.status.success(), "git {args:?} failed");
    out.stdout
}

fn write_file(dir: &Path, rel: &str, body: &str) {
    write_bytes(dir, rel, body.as_bytes());
}

fn write_bytes(dir: &Path, rel: &str, body: &[u8]) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, body).unwrap();
}

fn read_file(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap_or_default()
}

/// 파일의 idx번째 줄(0-기준)을 바꾼다 — "같은 줄을 양쪽이 고침" 셋업용.
fn set_line(dir: &Path, rel: &str, idx: usize, val: &str) {
    let s = read_file(dir, rel);
    let mut lines: Vec<String> = s.lines().map(str::to_string).collect();
    assert!(idx < lines.len(), "set_line: {rel} has no line {idx}");
    lines[idx] = val.to_string();
    write_file(dir, rel, &(lines.join("\n") + "\n"));
}

fn set_identity(dir: &Path, name: &str, email: &str) {
    git(dir, &["config", "user.name", name]);
    git(dir, &["config", "user.email", email]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    // 편집기가 뜨면 CI 에서 죽는다 — merge/pull 의 자동 메시지를 그대로 쓴다.
    git(dir, &["config", "core.editor", "true"]);
}

fn target(dir: &Path) -> Target {
    Target::Local(dir.to_path_buf())
}

// ═════════════════════════════════════════════════════════════════════════
// NO-LOSS 불변식 검사기
// ═════════════════════════════════════════════════════════════════════════

struct Snapshot {
    /// 상대경로 → 내용 바이트 (.git 제외, tracked+untracked 전부).
    files: BTreeMap<String, Vec<u8>>,
    /// 상대경로 → 그 내용의 blob SHA.
    file_blob: BTreeMap<String, String>,
    /// refs/heads + refs/remotes + refs/tags + HEAD 에서 도달 가능한 커밋 전부.
    commits: BTreeSet<String>,
    /// (stash SHA, 그 항목이 운반하는 blob 집합) — worktree/index/untracked 3면.
    stashes: Vec<(String, BTreeSet<String>)>,
    /// 스냅샷 시점에 unmerged(충돌) 상태였던 경로 → 그 stage blob 들(:1/:2/:3).
    /// 이 경로의 워크트리 내용은 마커 합성본(스크래치)으로 취급한다 (모듈 doc).
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

/// `git hash-object --stdin` — `-w` 없이. 스냅샷 시점에 blob 을 ODB 에 써
/// 넣으면 "여전히 존재한다" 검사가 무의미해진다.
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

/// 도달 가능성 검사의 뿌리(root)들. `generous == false`(스냅샷 시점)는
/// refs + HEAD 만 — "팀원이 이름으로 가리킬 수 있는 것". `generous == true`
/// (사후 검증)는 MERGE_HEAD 도 인정한다. ORIG_HEAD 와 reflog 는 양쪽 다
/// 제외 (모듈 doc 참고 — 한 칸/90일짜리 생존은 손실로 간주).
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

/// root 들에서 도달 가능한 모든 오브젝트(커밋/트리/blob) SHA.
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

/// index 가 물고 있는 blob (충돌 중이면 stage 1/2/3 전부 나온다 — staged
/// 인데 커밋 안 된 내용은 여기에만 존재한다).
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

/// stash 항목 하나가 운반하는 blob: worktree 트리(<sha>) + index(<sha>^2)
/// + untracked(<sha>^3, `-u` 저장시에만 존재).
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

/// `git ls-files -u` — 충돌 중인 경로와 stage blob 들.
fn unmerged_stages(repo: &Path) -> BTreeMap<String, Vec<String>> {
    let out = git_try(repo, &["ls-files", "-u"]);
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        // "<mode> <sha> <stage>\t<path>"
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

fn snapshot(repo: &Path) -> Snapshot {
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
    Snapshot {
        files,
        file_blob,
        commits,
        stashes,
        unmerged: unmerged_stages(repo),
    }
}

fn verify_no_loss(repo: &Path, before: &Snapshot, label: &str, allow_stash_vanish: bool) {
    // 사후 root: refs + HEAD + MERGE_HEAD + 모든 stash SHA (ORIG_HEAD 제외).
    let mut roots = commit_roots(repo, true);
    let after_stash = stash_shas(repo);
    roots.extend(after_stash.iter().cloned());
    roots.sort();
    roots.dedup();

    // 1) 커밋: 이전에 도달 가능했던 모든 커밋이 여전히 도달 가능해야 한다.
    let after_commits = rev_list_commits(repo, &roots);
    let lost: Vec<&String> = before.commits.difference(&after_commits).collect();
    assert!(
        lost.is_empty(),
        "[NO-LOSS 위반] op='{label}': 커밋 {}개가 더 이상 어떤 ref/HEAD/MERGE_HEAD/stash 에서도 도달 불가: {:?}",
        lost.len(),
        lost
    );

    // 2) "찾을 수 있는" blob 집합: 도달 가능한 오브젝트 + index + 현재 워크트리.
    let mut findable = rev_list_objects(repo, &roots);
    findable.extend(index_blobs(repo));
    let now_files = walk_files(repo);
    for bytes in now_files.values() {
        findable.insert(hash_bytes(repo, bytes));
    }

    // 3) 파일: 스냅샷의 모든 내용이 여전히 찾을 수 있어야 한다.
    for (path, bytes) in &before.files {
        if now_files.get(path) == Some(bytes) {
            continue; // 같은 자리에 그대로.
        }
        let sha = &before.file_blob[path];
        if findable.contains(sha) {
            continue; // 커밋/스태시/index/워크트리 어딘가에 있음.
        }
        // 충돌 마커 합성본이었던 경로: 합성본 대신 충돌의 각 stage(:1/:2/:3)
        // 가 계속 찾을 수 있으면 통과 (모듈 doc 의 예외 규칙).
        if let Some(stage_blobs) = before.unmerged.get(path) {
            let missing: Vec<&String> =
                stage_blobs.iter().filter(|b| !findable.contains(*b)).collect();
            assert!(
                missing.is_empty(),
                "[NO-LOSS 위반] op='{label}': 충돌 중이던 '{path}' 의 stage blob {missing:?} \
                 (충돌의 한쪽/base)이 더 이상 찾을 수 없다"
            );
            continue;
        }
        let head = &bytes[..bytes.len().min(80)];
        panic!(
            "[NO-LOSS 위반] op='{label}' 가 파일 내용을 잃었다: '{path}' \
             (blob {sha}, {}바이트, 시작: {:?}) — 워크트리 어디에도 없고 \
             ref/stash/index/MERGE_HEAD 에서 도달 불가",
            bytes.len(),
            String::from_utf8_lossy(head)
        );
    }

    // 4) stash: 사라진 항목은 내용 blob 이 전부 찾을 수 있어야 한다
    //    (성공한 pop). 명시적 drop 만 allow_stash_vanish 로 면제.
    let after_stash_set: BTreeSet<&String> = after_stash.iter().collect();
    for (sha, blobs) in &before.stashes {
        if after_stash_set.contains(sha) || allow_stash_vanish {
            continue;
        }
        for b in blobs {
            assert!(
                findable.contains(b),
                "[NO-LOSS 위반] op='{label}' 가 stash {sha} 를 지웠는데 \
                 그 안의 blob {b} 를 더 이상 찾을 수 없다"
            );
        }
    }
}

/// 불변식 검사 래퍼 — 스냅샷 → 실행 → 검증. 성공/실패(Result 의 Err 포함)
/// 어느 쪽이든 검증은 항상 수행되고, 손실이 있으면 label 과 함께 panic.
fn checked<T>(repo: &Path, label: &str, op: impl FnOnce() -> T) -> T {
    let before = snapshot(repo);
    let out = op();
    verify_no_loss(repo, &before, label, false);
    out
}

/// 사용자가 명시적으로 stash 항목을 버리는 한 가지 경우만 면제하는 변형.
fn checked_stash_drop<T>(repo: &Path, label: &str, op: impl FnOnce() -> T) -> T {
    let before = snapshot(repo);
    let out = op();
    verify_no_loss(repo, &before, label, true);
    out
}

// ═════════════════════════════════════════════════════════════════════════
// 팀 리그 (bare origin + 관리자 + 팀원 클론)
// ═════════════════════════════════════════════════════════════════════════

struct Rig {
    _bare: TempDir,
    bare: PathBuf,
    url: String,
}

/// bare origin 을 만들고, 관리자가 seed 파일로 첫 커밋을 push 한다.
fn new_rig(seed: &[(&str, &str)]) -> (Rig, TempDir) {
    let bare = TempDir::new().unwrap();
    git(bare.path(), &["init", "--bare", "-q", "-b", "main"]);
    let url = format!("file://{}", bare.path().display());
    let mgr = TempDir::new().unwrap();
    git(mgr.path(), &["init", "-q", "-b", "main"]);
    set_identity(mgr.path(), "관리자", "manager@t.com");
    for (p, b) in seed {
        write_file(mgr.path(), p, b);
    }
    git(mgr.path(), &["add", "-A"]);
    git(mgr.path(), &["commit", "-q", "-m", "init"]);
    git(mgr.path(), &["remote", "add", "origin", &url]);
    git(mgr.path(), &["push", "-q", "-u", "origin", "main"]);
    let rig = Rig {
        bare: bare.path().to_path_buf(),
        _bare: bare,
        url,
    };
    (rig, mgr)
}

fn clone_member(rig: &Rig, name: &str, email: &str) -> TempDir {
    let d = TempDir::new().unwrap();
    git(d.path(), &["clone", "-q", &rig.url, "."]);
    set_identity(d.path(), name, email);
    d
}

/// 관리자가 main 위에 파일 하나를 고쳐 커밋+push (팀원과 충돌 유발용).
fn manager_commit_main(mgr: &Path, rel: &str, body: &str, msg: &str) {
    write_file(mgr, rel, body);
    git(mgr, &["add", "-A"]);
    git(mgr, &["commit", "-q", "-m", msg]);
    git(mgr, &["push", "-q", "origin", "main"]);
}

/// 관리자가 팀원 브랜치를 main 에 병합하고 push (raw git — 관리자 역할 재현).
fn manager_merge_and_push(mgr: &Path, branch: &str) {
    git(mgr, &["fetch", "-q", "origin"]);
    git(
        mgr,
        &["merge", "--no-ff", "--no-edit", "-q", &format!("origin/{branch}")],
    );
    git(mgr, &["push", "-q", "origin", "main"]);
}

// ═════════════════════════════════════════════════════════════════════════
// 시나리오 0 — 검사기 자기 검증: "green" 이 의미를 가지려면 검사기가
//              실제 손실을 무조건 잡는다는 것부터 증명해야 한다.
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn s0_checker_self_test_detects_real_losses() {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    let (rig, _mgr) = new_rig(&[("app.txt", "v1\n")]);
    let mem = clone_member(&rig, "팀원", "member@t.com");
    let m = mem.path();

    // (a) untracked 파일이 통째로 사라지면 → 잡는다.
    write_file(m, "precious.txt", "커밋한 적 없는 소중한 작업\n");
    let r = catch_unwind(AssertUnwindSafe(|| {
        checked(m, "self-test: untracked 삭제", || {
            fs::remove_file(m.join("precious.txt")).unwrap();
        })
    }));
    assert!(r.is_err(), "untracked 파일 증발을 검사기가 놓쳤다");
    write_file(m, "precious.txt", "커밋한 적 없는 소중한 작업\n"); // 복구.

    // (b) 커밋이 reflog 에만 남게 되면(reset --hard) → 잡는다.
    //     (reflog-생존은 손실로 간주 — 모듈 doc 의 엄격성 결정.)
    git(m, &["add", "-A"]);
    git(m, &["commit", "-q", "-m", "droppable"]);
    let r = catch_unwind(AssertUnwindSafe(|| {
        checked(m, "self-test: reset --hard 로 커밋 증발", || {
            git(m, &["reset", "-q", "--hard", "HEAD~1"]);
        })
    }));
    assert!(r.is_err(), "커밋 증발(reset --hard)을 검사기가 놓쳤다");

    // (c) stash 항목이 내용과 함께 사라지면 → 잡는다. 명시적 drop 래퍼는 면제.
    write_file(m, "app.txt", "stash 로 갈 내용\n");
    git(m, &["stash", "push", "-q", "-u"]);
    let r = catch_unwind(AssertUnwindSafe(|| {
        checked(m, "self-test: 무단 stash drop", || {
            git(m, &["stash", "drop", "-q", "stash@{0}"]);
        })
    }));
    assert!(r.is_err(), "stash 증발을 검사기가 놓쳤다");
    // (b)에서 이미 지워졌으므로 stash 는 남아 있지 않을 수 있다 — (c)의
    // drop 이 panic 전에 실제로 수행됐는지에 따라 잔여 상태가 다르다.
    // 면제 래퍼 경로: 새 stash 를 만들어 명시적 drop 은 통과해야 한다.
    write_file(m, "app.txt", "면제 검증용 내용\n");
    git(m, &["stash", "push", "-q", "-u"]);
    checked_stash_drop(m, "self-test: 명시적 drop 은 허용", || {
        git(m, &["stash", "drop", "-q", "stash@{0}"]);
    });
}

// ═════════════════════════════════════════════════════════════════════════
// 시나리오 1 — 정석 루프 3바퀴: 브랜치 → 편집 → 커밋 → 푸시 → (관리자) →
//              동기화 → 충돌 해결(manual/theirs) → 완료 → 계속 편집.
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn s1_canonical_loop_three_rounds() {
    let (rig, mgr) = new_rig(&[
        ("app.txt", "alpha\nbeta\ngamma\n"),
        ("docs/guide.md", "guide v1\n"),
    ]);
    let mem = clone_member(&rig, "팀원", "member@t.com");
    let m = mem.path();
    let t = target(m);

    checked(m, "r0 create_branch(feature/w)", || {
        create_branch(&t, "feature/w")
    })
    .unwrap();

    let mut member_commits: Vec<String> = Vec::new();

    for round in 1..=3usize {
        // ── 편집: tracked 수정 + 새 untracked 파일 + staged-미커밋 변경 동시 ──
        match round {
            1 => set_line(m, "app.txt", 0, "alpha-m1"),
            2 => set_line(m, "app.txt", 1, "beta-m2"),
            _ => set_line(m, "app.txt", 2, "gamma-m3"),
        }
        write_file(m, &format!("member_r{round}.txt"), &format!("round {round} 작업\n"));
        let guide = read_file(m, "docs/guide.md") + &format!("round {round} 메모\n");
        write_file(m, "docs/guide.md", &guide);
        checked(m, &format!("r{round} add(docs/guide.md)"), || {
            add(&t, &["docs/guide.md".to_string()])
        })
        .unwrap();

        // ── 커밋 + 푸시 (검사기 아래) ──
        let c = checked(m, &format!("r{round} commit"), || {
            commit(&t, &format!("member round {round}"), true)
        })
        .unwrap();
        assert!(c.ok, "round {round} 커밋 실패: {}", c.message);
        member_commits.push(c.sha.clone().unwrap());
        let p = checked(m, &format!("r{round} push"), || push(&t, None, None)).unwrap();
        assert!(p.ok, "round {round} 푸시 실패: {}", p.message);

        // ── 관리자 행동 (raw git, 별도 클론) ──
        match round {
            1 => manager_merge_and_push(mgr.path(), "feature/w"),
            2 => {
                // main 의 app.txt = "alpha-m1\nbeta\ngamma\n" (r1 병합 결과).
                set_line(mgr.path(), "app.txt", 1, "beta-manager");
                git(mgr.path(), &["add", "-A"]);
                git(mgr.path(), &["commit", "-q", "-m", "manager edits beta"]);
                git(mgr.path(), &["push", "-q", "origin", "main"]);
            }
            _ => {
                set_line(mgr.path(), "app.txt", 2, "gamma-manager");
                git(mgr.path(), &["add", "-A"]);
                git(mgr.path(), &["commit", "-q", "-m", "manager edits gamma"]);
                git(mgr.path(), &["push", "-q", "origin", "main"]);
            }
        }

        // ── 동기화 + 충돌 해결 (검사기 아래) ──
        let sr = checked(m, &format!("r{round} sync_to_base"), || {
            sync_to_base(&t, "main", "origin")
        })
        .unwrap();
        match round {
            1 => assert!(!sr.conflicted, "r1 은 충돌 없어야 함: {}", sr.message),
            2 => {
                assert!(sr.conflicted && sr.files.contains(&"app.txt".to_string()));
                let d = checked(m, "r2 conflict_detail", || conflict_detail(&t, "app.txt"))
                    .unwrap();
                // sync 는 origin/main 을 내 브랜치로 가져온다 → ours = 팀원.
                assert!(d.ours.contains("beta-m2"), "ours 는 팀원 쪽: {}", d.ours);
                assert!(d.theirs.contains("beta-manager"), "theirs 는 main 쪽");
                let remaining = checked(m, "r2 resolve manual", || {
                    resolve_conflict(
                        &t,
                        "app.txt",
                        &Resolution::Manual {
                            content: "alpha-m1\nbeta-m2+manager\ngamma\n".into(),
                        },
                    )
                })
                .unwrap();
                assert!(remaining.is_empty());
                let done =
                    checked(m, "r2 complete_merge", || complete_merge(&t, None)).unwrap();
                assert!(done.ok, "r2 병합 완료 실패: {}", done.message);
            }
            _ => {
                assert!(sr.conflicted && sr.files.contains(&"app.txt".to_string()));
                let d = checked(m, "r3 conflict_detail", || conflict_detail(&t, "app.txt"))
                    .unwrap();
                assert!(d.ours.contains("gamma-m3"));
                assert!(d.theirs.contains("gamma-manager"));
                let remaining = checked(m, "r3 resolve theirs", || {
                    resolve_conflict(&t, "app.txt", &Resolution::Theirs)
                })
                .unwrap();
                assert!(remaining.is_empty());
                let done = checked(m, "r3 complete_merge", || {
                    complete_merge(&t, Some("main 병합"))
                })
                .unwrap();
                assert!(done.ok);

                // FIXME(LOSS-1): `Resolution::Theirs` 는 `git checkout --theirs`
                // 로 **파일 전체**를 상대(stage :3) 버전으로 바꾼다. 이 파일에서
                // 팀원이 가진, 충돌과 무관한(자동 병합됐던) 변경 — r2 에서 손수
                // 병합한 line2 "beta-m2+manager" — 까지 함께 되돌아간다.
                // origin/main 은 그 줄을 "beta-manager" 로 갖고 있었으므로 병합
                // 결과의 line2 는 팀원의 해결 내용이 사라진 채 main 쪽이 된다.
                // 커밋 자체는 도달 가능해서(아래 검증) 불변식 위반은 아니지만,
                // 병합 결과(=곧 main 이 될 내용)에서 팀원의 작업이 소리 없이
                // 회귀한다. 최소 수정: side-pick 은 파일 전체 치환이 아니라
                // "충돌 블록만 그 쪽 선택"(자동 병합된 워크트리에서 마커 블록만
                // 치환)으로 바꾸거나, 자동 병합분이 날아간다는 경고를 띄울 것.
                let body = read_file(m, "app.txt");
                assert_eq!(
                    body, "alpha-m1\nbeta-manager\ngamma-manager\n",
                    "현재 동작: theirs 가 파일 전체를 덮어 r2 해결(beta-m2+manager)이 회귀한다"
                );
            }
        }

        // 동기화 결과(병합 커밋)를 공유 — 루프의 마지막 단계.
        let p = checked(m, &format!("r{round} push(동기화 후)"), || push(&t, None, None))
            .unwrap();
        assert!(p.ok, "동기화 후 푸시 실패: {}", p.message);

        // ── 계속 편집 (다음 라운드 커밋에 포함될 dirty 상태) ──
        write_file(m, "wip.txt", &format!("round {round} 이후 진행 중 작업\n"));
    }

    // 팀원의 모든 라운드 커밋이 여전히 도달 가능 (명시적 재확인).
    let all = git(m, &["rev-list", "HEAD"]);
    for sha in &member_commits {
        assert!(all.contains(sha), "팀원 커밋 {sha} 이 히스토리에서 사라짐");
    }

    // 관리자가 최종 병합하면 origin/main 에 팀원 작업 전부가 실린다.
    manager_merge_and_push(mgr.path(), "feature/w");
    let tree = git(&rig.bare, &["ls-tree", "-r", "--name-only", "main"]);
    for f in ["member_r1.txt", "member_r2.txt", "member_r3.txt", "docs/guide.md"] {
        assert!(tree.contains(f), "origin/main 트리에 {f} 가 없다");
    }
}

// ═════════════════════════════════════════════════════════════════════════
// 시나리오 2 — dirty 상태에서의 충돌: 거부 동작들 + stash 왕복 무손실.
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn s2_dirty_state_collisions() {
    let (rig, mgr) = new_rig(&[
        ("app.txt", "base line\n"),
        ("other.txt", "other v1\n"),
    ]);
    let mem = clone_member(&rig, "팀원", "member@t.com");
    let m = mem.path();
    let t = target(m);

    // 팀원: 브랜치에서 app.txt 를 고쳐 커밋+푸시 (main 과 달라짐).
    checked(m, "create_branch(feature/dirty)", || {
        create_branch(&t, "feature/dirty")
    })
    .unwrap();
    write_file(m, "app.txt", "base-member\n");
    let c = checked(m, "커밋", || commit(&t, "member base edit", true)).unwrap();
    assert!(c.ok);
    let p = checked(m, "푸시", || push(&t, None, None)).unwrap();
    assert!(p.ok, "{}", p.message);

    // 관리자: main 에서 같은 파일을 다르게 + 새 파일 shared_new.txt 추가.
    write_file(mgr.path(), "app.txt", "base-manager\n");
    write_file(mgr.path(), "shared_new.txt", "manager version\n");
    git(mgr.path(), &["add", "-A"]);
    git(mgr.path(), &["commit", "-q", "-m", "manager touches app + adds shared_new"]);
    git(mgr.path(), &["push", "-q", "origin", "main"]);

    // 동료: 팀원의 브랜치에도 겹치는 push (pull 거부 유발용).
    let col = clone_member(&rig, "동료", "col@t.com");
    git(col.path(), &["checkout", "-q", "feature/dirty"]);
    write_file(col.path(), "app.txt", "base-colleague\n");
    git(col.path(), &["add", "-A"]);
    git(col.path(), &["commit", "-q", "-m", "colleague overlaps"]);
    git(col.path(), &["push", "-q", "origin", "feature/dirty"]);

    // ── 팀원의 3중 dirty 상태: tracked 미스테이지 + untracked + staged ──
    write_file(m, "app.txt", "dirty-uncommitted-edit\n"); // tracked, unstaged, 겹침
    write_file(m, "note.txt", "아직 커밋 안 한 새 메모\n"); // untracked
    write_file(m, "shared_new.txt", "member's own version\n"); // untracked, 원격과 이름 충돌
    write_file(m, "other.txt", "other v2 staged\n");
    checked(m, "add(other.txt) → staged 상태 준비", || {
        add(&t, &["other.txt".to_string()])
    })
    .unwrap();

    let dirty_state = walk_files(m); // 이후 "아무것도 안 건드림" 비교 기준.
    let head_before = git(m, &["rev-parse", "HEAD"]);

    // 1) sync_to_base — 거부해야 하고 아무것도 건드리면 안 된다.
    let err = checked(m, "sync_to_base(dirty)", || sync_to_base(&t, "main", "origin"))
        .unwrap_err();
    assert!(
        err.to_string().contains("커밋"),
        "친절한 dirty 거부 메시지여야 한다: {err}"
    );
    assert_eq!(walk_files(m), dirty_state, "sync 거부가 워크트리를 건드렸다");
    assert_eq!(git(m, &["rev-parse", "HEAD"]), head_before);
    assert!(!merge_in_progress(&t).unwrap());

    // 2) checkout_branch — 겹치는 dirty 로 거부.
    let err = checked(m, "checkout_branch(main, dirty)", || {
        checkout_branch(&t, "main")
    })
    .unwrap_err();
    assert!(err.to_string().contains("커밋되지 않은 변경사항"), "{err}");
    assert_eq!(walk_files(m), dirty_state);

    // 3) pull — fetch 는 되고 merge 는 시작 전에 거부된다. 아무것도 안 잃음.
    // 회귀 방지(UX-5): friendly_git_error 가 dirty-tree 패턴을 한국어로
    // 바꾼다 — 영어 원문("would be overwritten") 노출 금지.
    let out = checked(m, "pull(dirty, 원격 갱신 있음)", || pull(&t)).unwrap();
    assert!(!out.ok, "겹치는 dirty 상태의 pull 은 실패해야 한다");
    assert!(out.conflicted_files.is_empty(), "머지 시작 전 거부 — 충돌 아님");
    assert!(
        out.message.contains("커밋하거나 스태시"),
        "한국어 안내여야 한다: {}",
        out.message
    );
    assert!(!out.message.contains("overwritten"), "{}", out.message);
    assert_eq!(walk_files(m), dirty_state, "pull 거부가 워크트리를 건드렸다");

    // 4) start_merge — 거부.
    let err = checked(m, "start_merge(dirty)", || {
        start_merge(&t, "origin/main", "main", "origin", None)
    })
    .unwrap_err();
    assert!(err.to_string().contains("커밋"), "{err}");
    assert_eq!(walk_files(m), dirty_state);

    // 5) stash save -u → 내용이 stash 에서 찾을 수 있어야 한다 (검사기 검증).
    checked(m, "stash save -u", || {
        stash(&t, StashAction::Save { message: Some("dirty 왕복".into()) })
    })
    .unwrap();
    assert_eq!(list_stashes(&t).unwrap().len(), 1);
    // untracked 였던 note.txt 내용 blob 이 stash 어딘가에 있는지 명시 확인.
    let note_blob = hash_bytes(m, "아직 커밋 안 한 새 메모\n".as_bytes());
    let entry = stash_shas(m)[0].clone();
    assert!(
        stash_content_blobs(m, &entry).contains(&note_blob),
        "untracked 내용이 stash 에 없다"
    );

    // 6) checkout 왕복 + pop → 바이트 단위로 원상복구.
    checked(m, "checkout main(정리 후)", || checkout_branch(&t, "main")).unwrap();
    checked(m, "checkout feature/dirty 복귀", || {
        checkout_branch(&t, "feature/dirty")
    })
    .unwrap();
    checked(m, "stash pop", || stash(&t, StashAction::Pop)).unwrap();
    assert_eq!(
        walk_files(m),
        dirty_state,
        "stash 왕복 후 워크트리가 바이트 단위로 같아야 한다"
    );
    assert!(list_stashes(&t).unwrap().is_empty());

    // FIXME(UX-8): 내용은 완전 복원되지만 other.txt 의 "staged" 표시는
    // 사라진다 — pop 이 `--index` 없이 수행되기 때문 (src/git/ops.rs:594).
    // 내용 손실은 아니고 메타데이터 손실. 최소 수정: Save 로 만든 항목을
    // 되살릴 때 `git stash apply --index` 를 먼저 시도.
    let st = list_status(&t).unwrap();
    let other = st.files.iter().find(|f| f.path == "other.txt").unwrap();
    assert!(!other.staged, "현재 동작: pop 후 staged 플래그는 유실된다");
}

// ═════════════════════════════════════════════════════════════════════════
// 시나리오 3 — stash 경계: pop 충돌 시 양쪽 내용 모두 보존 + 명시적 drop.
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn s3_stash_pop_conflict_keeps_both_sides() {
    let (rig, _mgr) = new_rig(&[("work.txt", "v1\n")]);
    let mem = clone_member(&rig, "팀원", "member@t.com");
    let m = mem.path();
    let t = target(m);

    // stash 에 A 버전을 보관.
    write_file(m, "work.txt", "A-version\n");
    checked(m, "stash save(A)", || {
        stash(&t, StashAction::Save { message: Some("A 작업".into()) })
    })
    .unwrap();
    let a_blob = hash_bytes(m, "A-version\n".as_bytes());

    // 충돌하게 만들 B 버전을 커밋.
    write_file(m, "work.txt", "B-version\n");
    let c = checked(m, "commit(B)", || commit(&t, "B edit", true)).unwrap();
    assert!(c.ok);

    // pop → 충돌로 실패. stash 항목은 남아 있어야 한다.
    let err = checked(m, "stash pop(충돌)", || stash(&t, StashAction::Pop)).unwrap_err();
    assert!(
        err.to_string().contains("충돌") && err.to_string().contains("남아"),
        "충돌 + 항목 보존 안내여야 한다: {err}"
    );
    assert_eq!(list_stashes(&t).unwrap().len(), 1, "충돌한 pop 은 항목을 지우면 안 된다");

    // 양쪽 모두 찾을 수 있어야 한다: 워크트리(마커 포함)에 A/B 둘 다 +
    // 원본 A 는 stash blob 으로도.
    let body = read_file(m, "work.txt");
    assert!(body.contains("A-version") && body.contains("B-version"), "{body}");
    let entry = stash_shas(m)[0].clone();
    assert!(stash_content_blobs(m, &entry).contains(&a_blob));

    // 회귀 방지(BUG-2): MERGE_HEAD 없이 남은 unmerged 항목(스태시 복원
    // 충돌)이 있으면 동기화는 시작 전에 거부된다 — 남의 충돌이 "동기화
    // 충돌"로 둔갑해 병합 커밋 아닌 일반 커밋으로 이어지던 오인 차단.
    let err = checked(m, "sync during stash-pop conflict", || {
        sync_to_base(&t, "main", "origin")
    })
    .unwrap_err();
    assert!(err.to_string().contains("해결되지 않은 충돌"), "{err}");
    assert!(!merge_in_progress(&t).unwrap());

    // 손으로 해결(앱 API 로 파일 저장) → add → 커밋.
    checked(m, "write merged body", || {
        write_file_at_target(&t, "work.txt", "B-version\nA-version\n".as_bytes())
    })
    .unwrap();
    checked(m, "add resolution", || add(&t, &["work.txt".to_string()])).unwrap();
    let c = checked(m, "commit resolution", || commit(&t, "A/B 손 해결", true)).unwrap();
    assert!(c.ok, "{}", c.message);

    // 명시적 drop — 사용자가 스스로 버리는 유일한 경로. 면제 래퍼 사용.
    checked_stash_drop(m, "stash drop(명시적)", || {
        stash(&t, StashAction::DropIndex("stash@{0}".into()))
    })
    .unwrap();
    assert!(list_stashes(&t).unwrap().is_empty());
    // A 의 내용은 해결 커밋 안에 살아 있다.
    assert!(git(m, &["show", "HEAD:work.txt"]).contains("A-version"));
}

// ═════════════════════════════════════════════════════════════════════════
// 시나리오 4 — 작업 중 동기화 충돌: 미푸시 커밋 2개 + 미커밋 편집 → 커밋 →
//              sync 충돌 → 양쪽을 합친 manual 해결 → 아무것도 잃지 않음.
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn s4_sync_conflict_midwork_manual_merges_both_sides() {
    let (rig, mgr) = new_rig(&[("app.txt", "one\ntwo\nthree\n")]);
    let mem = clone_member(&rig, "팀원", "member@t.com");
    let m = mem.path();
    let t = target(m);

    checked(m, "create_branch(feature/mid)", || {
        create_branch(&t, "feature/mid")
    })
    .unwrap();

    // 미푸시 커밋 2개.
    write_file(m, "feature_a.txt", "feature a\n");
    let c1 = checked(m, "c1", || commit(&t, "c1: feature_a", true)).unwrap();
    assert!(c1.ok);
    set_line(m, "app.txt", 0, "one-member");
    let c2 = checked(m, "c2", || commit(&t, "c2: app one-member", true)).unwrap();
    assert!(c2.ok);

    // 미커밋 편집 → 커밋해서 c3.
    set_line(m, "app.txt", 2, "three-member");
    write_file(m, "scratch.txt", "생각 정리\n");
    let c3 = checked(m, "c3(미커밋분 커밋)", || commit(&t, "c3: wip", true)).unwrap();
    assert!(c3.ok);

    // 관리자: main 에서 같은 첫 줄을 다르게 고쳐 push.
    set_line(mgr.path(), "app.txt", 0, "one-manager");
    git(mgr.path(), &["add", "-A"]);
    git(mgr.path(), &["commit", "-q", "-m", "manager one"]);
    git(mgr.path(), &["push", "-q", "origin", "main"]);

    // 동기화 → 충돌 → 양쪽을 합친 manual 해결 → 완료.
    let sr = checked(m, "sync_to_base(충돌)", || sync_to_base(&t, "main", "origin")).unwrap();
    assert!(sr.conflicted && sr.files.contains(&"app.txt".to_string()));
    let d = checked(m, "conflict_detail", || conflict_detail(&t, "app.txt")).unwrap();
    assert!(d.ours.contains("one-member") && d.theirs.contains("one-manager"));
    let remaining = checked(m, "resolve manual(both)", || {
        resolve_conflict(
            &t,
            "app.txt",
            &Resolution::Manual {
                content: "one-member\none-manager\ntwo\nthree-member\n".into(),
            },
        )
    })
    .unwrap();
    assert!(remaining.is_empty());
    let done = checked(m, "complete_merge", || complete_merge(&t, None)).unwrap();
    assert!(done.ok, "{}", done.message);

    // 양쪽 내용이 모두 있고, 세 커밋 모두 도달 가능.
    let body = git(m, &["show", "HEAD:app.txt"]);
    assert!(body.contains("one-member") && body.contains("one-manager"), "{body}");
    assert!(body.contains("three-member"));
    let all = git(m, &["rev-list", "HEAD"]);
    for (label, sha) in [("c1", &c1.sha), ("c2", &c2.sha), ("c3", &c3.sha)] {
        assert!(
            all.contains(sha.as_ref().unwrap()),
            "{label} 커밋이 히스토리에서 사라졌다"
        );
    }
}

// ═════════════════════════════════════════════════════════════════════════
// 시나리오 5 — 원격 브랜치가 발밑에서 삭제됨 (완전 병합 후 정리):
//              새 로컬 커밋 + 미커밋 편집 보유 → prune fetch → 상태 조회 →
//              push 가 원격 브랜치를 재생성. 어느 단계에서도 무손실.
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn s5_remote_branch_deleted_under_member() {
    let (rig, mgr) = new_rig(&[("app.txt", "v1\n")]);
    let mem = clone_member(&rig, "팀원", "member@t.com");
    let m = mem.path();
    let t = target(m);

    checked(m, "create_branch(feature/gone)", || {
        create_branch(&t, "feature/gone")
    })
    .unwrap();
    write_file(m, "gone_work.txt", "정리될 브랜치의 작업\n");
    let c = checked(m, "커밋", || commit(&t, "gone work", true)).unwrap();
    assert!(c.ok);
    let p = checked(m, "푸시", || push(&t, None, None)).unwrap();
    assert!(p.ok);

    // 관리자: 병합 + push + (앱 API 로) 원격 브랜치 삭제 — 관리자 저장소도
    // 검사기로 감싸 관리자 쪽 손실이 없는지 함께 본다.
    manager_merge_and_push(mgr.path(), "feature/gone");
    let mgr_t = target(mgr.path());
    checked(mgr.path(), "delete_remote_branch(feature/gone)", || {
        delete_remote_branch(&mgr_t, "origin", "main", "feature/gone")
    })
    .unwrap();

    // 팀원(삭제 사실 모름): 새 커밋 2개 + 미커밋 편집.
    write_file(m, "gone2.txt", "삭제 후 새 작업 1\n");
    let c2 = checked(m, "새 커밋 1", || commit(&t, "post-delete 1", true)).unwrap();
    assert!(c2.ok);
    write_file(m, "gone3.txt", "삭제 후 새 작업 2\n");
    let c3 = checked(m, "새 커밋 2", || commit(&t, "post-delete 2", true)).unwrap();
    assert!(c3.ok);
    let dirty = "아직 커밋 안 한 추가 메모\n";
    write_file(m, "gone_work.txt", dirty);

    // prune fetch — 원격 트래킹 ref 가 사라져도 로컬 커밋/편집은 무손실.
    checked(m, "fetch --prune(브랜치 삭제 반영)", || fetch_target(&t, "origin")).unwrap();
    assert!(
        !git_try(m, &["rev-parse", "-q", "--verify", "refs/remotes/origin/feature/gone"])
            .status
            .success(),
        "prune 후 원격 트래킹 ref 는 없어야 한다"
    );

    // 상태는 계속 읽을 수 있고, ahead 는 base 기준으로 새 커밋 2개.
    let st = checked(m, "list_status", || list_status(&t)).unwrap();
    assert_eq!(st.branch.as_deref(), Some("feature/gone"));
    let st = checked(m, "list_status_with_base", || {
        list_status_with_base(&t, "main")
    })
    .unwrap();
    assert_eq!(st.ahead, 2, "upstream 이 사라져도 base 기준 미푸시 수를 보여야 한다");

    // push 가 원격 브랜치를 재생성한다.
    let p = checked(m, "push(재생성)", || push(&t, None, None)).unwrap();
    assert!(p.ok, "삭제된 원격 브랜치로의 push 는 재생성이어야 한다: {}", p.message);
    let remote_sha = git(&rig.bare, &["rev-parse", "refs/heads/feature/gone"]);
    assert_eq!(remote_sha.trim(), git(m, &["rev-parse", "HEAD"]).trim());
    assert_eq!(read_file(m, "gone_work.txt"), dirty, "미커밋 편집이 사라졌다");
}

// ═════════════════════════════════════════════════════════════════════════
// 시나리오 6 — non-FF 거부: 남이 내 브랜치에 push → 내 push 거부(친절 안내)
//              → pull 병합 → 양쪽 커밋 보존 → push 성공.
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn s6_non_ff_rejection_then_pull_then_push() {
    let (rig, _mgr) = new_rig(&[("app.txt", "v1\n")]);
    let mem = clone_member(&rig, "팀원", "member@t.com");
    let m = mem.path();
    let t = target(m);

    checked(m, "create_branch(feature/shared)", || {
        create_branch(&t, "feature/shared")
    })
    .unwrap();
    write_file(m, "seed.txt", "공유 브랜치 시작\n");
    let c = checked(m, "seed 커밋", || commit(&t, "seed", true)).unwrap();
    assert!(c.ok);
    let p = checked(m, "seed 푸시", || push(&t, None, None)).unwrap();
    assert!(p.ok);

    // 동료가 같은 브랜치에 자기 커밋을 push.
    let col = clone_member(&rig, "동료", "col@t.com");
    git(col.path(), &["checkout", "-q", "feature/shared"]);
    write_file(col.path(), "col.txt", "동료의 작업\n");
    git(col.path(), &["add", "-A"]);
    git(col.path(), &["commit", "-q", "-m", "colleague work"]);
    git(col.path(), &["push", "-q", "origin", "feature/shared"]);
    let col_sha = git(col.path(), &["rev-parse", "HEAD"]).trim().to_string();

    // 팀원도 로컬 커밋을 하나 만들고 push → non-FF 거부.
    write_file(m, "mem.txt", "팀원의 작업\n");
    let mine = checked(m, "내 커밋", || commit(&t, "member work", true)).unwrap();
    assert!(mine.ok);
    let rejected = checked(m, "push(non-FF 거부)", || push(&t, None, None)).unwrap();
    assert!(!rejected.ok, "낡은 로컬에서의 push 는 거부돼야 한다");
    // 회귀 방지(UX-4): non-FF 거부의 처방은 ‘풀’이다 — ‘동기화’는
    // origin/main 만 병합해 내 브랜치의 원격 커밋은 안 가져온다.
    assert!(
        rejected.message.contains("풀"),
        "‘풀’ 처방이어야 한다: {}",
        rejected.message
    );
    assert!(!rejected.message.contains("동기화"), "{}", rejected.message);

    // pull 이 병합해 주고, 양쪽 커밋이 모두 보존된다.
    let out = checked(m, "pull(병합)", || pull(&t)).unwrap();
    assert!(out.ok, "겹치지 않는 non-FF pull 은 병합 성공: {}", out.message);
    let all = git(m, &["rev-list", "HEAD"]);
    assert!(all.contains(&col_sha), "동료 커밋이 병합 후 도달 가능해야 한다");
    assert!(all.contains(mine.sha.as_ref().unwrap()));

    let p = checked(m, "push(재시도 성공)", || push(&t, None, None)).unwrap();
    assert!(p.ok, "{}", p.message);
    let tree = git(&rig.bare, &["ls-tree", "-r", "--name-only", "feature/shared"]);
    assert!(tree.contains("col.txt") && tree.contains("mem.txt"));
}

// ═════════════════════════════════════════════════════════════════════════
// 시나리오 7 — "크래시" 시뮬레이션: 충돌 절반 해결 상태를 디스크만으로 복원.
//
// kill -9 를 인프로세스로 재현할 수는 없다. 대신: 백엔드의 모든 함수는
// git CLI 를 부르는 무상태 래퍼라서(모듈 doc 참고) "재시작 후 첫 호출" ≡
// "그냥 새 호출"이다. 아래에서 부분 해결 상태를 만든 뒤, 이전 호출의 어떤
// 반환값도 재사용하지 않고 fresh 호출만으로 정확한 상황(진행 중 여부 /
// 남은 파일 / 각 파일의 3면)을 복원해 병합을 끝낸다. 중간의 매 단계가
// checked 로 감싸져 있으므로 "어느 단계 직후에 죽어도" 그 시점의 디스크
// 상태에 팀원의 모든 것이 남아 있음은 이미 기계적으로 검증된다.
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn s7_crash_mid_conflict_resumes_from_disk_alone() {
    let (rig, mgr) = new_rig(&[("app.txt", "a1\n"), ("config.txt", "c1\n")]);
    let mem = clone_member(&rig, "팀원", "member@t.com");
    let m = mem.path();
    let t = target(m);

    checked(m, "create_branch", || create_branch(&t, "feature/crash")).unwrap();
    write_file(m, "app.txt", "a1-member\n");
    write_file(m, "config.txt", "c1-member\n");
    let c = checked(m, "커밋", || commit(&t, "member edits both", true)).unwrap();
    assert!(c.ok);

    write_file(mgr.path(), "app.txt", "a1-manager\n");
    write_file(mgr.path(), "config.txt", "c1-manager\n");
    git(mgr.path(), &["add", "-A"]);
    git(mgr.path(), &["commit", "-q", "-m", "manager edits both"]);
    git(mgr.path(), &["push", "-q", "origin", "main"]);

    let sr = checked(m, "sync(2파일 충돌)", || sync_to_base(&t, "main", "origin")).unwrap();
    assert!(sr.conflicted);
    assert_eq!(sr.files.len(), 2, "{:?}", sr.files);

    // 한 파일만 해결(스테이징됨), 다른 파일은 미해결 — 여기서 "죽는다".
    let remaining = checked(m, "resolve app.txt(절반)", || {
        resolve_conflict(&t, "app.txt", &Resolution::Manual { content: "a1-merged\n".into() })
    })
    .unwrap();
    assert_eq!(remaining, vec!["config.txt".to_string()]);

    // ── "재시작": 위의 어떤 값도 쓰지 않고 fresh 호출만으로 재구성 ──
    let in_progress = checked(m, "재시작: merge_in_progress", || merge_in_progress(&t)).unwrap();
    assert!(in_progress, "MERGE_HEAD 가 디스크에 있으니 진행 중으로 복원돼야 한다");
    let remaining = checked(m, "재시작: remaining_conflicts", || remaining_conflicts(&t)).unwrap();
    assert_eq!(remaining, vec!["config.txt".to_string()], "해결분/미해결분 구분 복원");
    let d = checked(m, "재시작: conflict_detail", || conflict_detail(&t, "config.txt")).unwrap();
    assert!(d.ours.contains("c1-member") && d.theirs.contains("c1-manager"));
    // 절반 해결해 둔 파일 내용도 디스크(index+워크트리)에 그대로.
    assert_eq!(read_file(m, "app.txt"), "a1-merged\n");

    // 이어서 마무리.
    let remaining = checked(m, "재시작 후 resolve config.txt", || {
        resolve_conflict(&t, "config.txt", &Resolution::Ours)
    })
    .unwrap();
    assert!(remaining.is_empty());
    let done = checked(m, "재시작 후 complete_merge", || complete_merge(&t, None)).unwrap();
    assert!(done.ok, "{}", done.message);
    assert!(!merge_in_progress(&t).unwrap());
    assert_eq!(read_file(m, "app.txt"), "a1-merged\n");
    assert_eq!(read_file(m, "config.txt"), "c1-member\n"); // Ours 선택 결과.
}

// ═════════════════════════════════════════════════════════════════════════
// 시나리오 8 — 적대적 타이밍: 병합 중 fetch/pull/sync 가드, detached HEAD,
//              빈 커밋 메시지, 푸시할 것 없음, 연속 동기화 멱등성.
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn s8_hostile_timing_guards() {
    let (rig, mgr) = new_rig(&[("app.txt", "a\n"), ("config.txt", "c\n")]);
    let mem = clone_member(&rig, "팀원", "member@t.com");
    let m = mem.path();
    let t = target(m);

    checked(m, "create_branch", || create_branch(&t, "feature/timing")).unwrap();
    write_file(m, "app.txt", "a-member\n");
    write_file(m, "config.txt", "c-member\n");
    let c = checked(m, "커밋", || commit(&t, "member", true)).unwrap();
    assert!(c.ok);
    write_file(mgr.path(), "app.txt", "a-manager\n");
    write_file(mgr.path(), "config.txt", "c-manager\n");
    git(mgr.path(), &["add", "-A"]);
    git(mgr.path(), &["commit", "-q", "-m", "manager"]);
    git(mgr.path(), &["push", "-q", "origin", "main"]);

    // 2파일 충돌 상태 + 한 파일만 해결 (부분 진행 상태에서 가드 검사).
    let sr = checked(m, "sync(충돌)", || sync_to_base(&t, "main", "origin")).unwrap();
    assert!(sr.conflicted && sr.files.len() == 2);
    checked(m, "app.txt 해결", || {
        resolve_conflict(&t, "app.txt", &Resolution::Manual { content: "a-merged\n".into() })
    })
    .unwrap();

    // (a) 병합 중 fetch — 허용되고 아무것도 잃지 않는다.
    checked(m, "MERGE_HEAD 중 fetch", || fetch_target(&t, "origin")).unwrap();
    assert!(merge_in_progress(&t).unwrap());
    assert_eq!(read_file(m, "app.txt"), "a-merged\n", "해결분이 살아 있어야 한다");

    // (b) 병합 중 pull — 실패하고 상태는 그대로. 남은 충돌 파일을 되돌려
    //     주는데, 이는 pull 이 만든 충돌이 아니라 기존 병합의 것이다
    //     (UI 가 병합 탭으로 보내므로 실질 피해는 없음 — 정상동작확인,
    //     다만 메시지가 "충돌이 발생했습니다"로 새 사건처럼 읽히는 뉘앙스).
    let out = checked(m, "MERGE_HEAD 중 pull", || pull(&t)).unwrap();
    assert!(!out.ok);
    assert_eq!(out.conflicted_files, vec!["config.txt".to_string()]);
    assert!(merge_in_progress(&t).unwrap(), "pull 이 MERGE_HEAD 를 지우면 안 된다");
    assert_eq!(read_file(m, "app.txt"), "a-merged\n");

    // (c) 병합 중 sync — 명시적 거부.
    let err = checked(m, "MERGE_HEAD 중 sync", || sync_to_base(&t, "main", "origin"))
        .unwrap_err();
    assert!(err.to_string().contains("진행 중인 병합"), "{err}");

    // 마무리하고 push.
    checked(m, "config.txt 해결", || {
        resolve_conflict(&t, "config.txt", &Resolution::Ours)
    })
    .unwrap();
    let done = checked(m, "complete_merge", || complete_merge(&t, None)).unwrap();
    assert!(done.ok);
    let p = checked(m, "push", || push(&t, None, None)).unwrap();
    assert!(p.ok, "{}", p.message);

    // (d) detached HEAD — sync/push 모두 거부 (병합 커밋 미아 방지 가드).
    git(m, &["checkout", "-q", "--detach"]);
    let err = checked(m, "detached sync", || sync_to_base(&t, "main", "origin")).unwrap_err();
    assert!(err.to_string().contains("브랜치 위에 있지 않습니다"), "{err}");
    let err = checked(m, "detached push", || push(&t, None, None)).unwrap_err();
    assert!(err.to_string().contains("브랜치 위에 있지 않습니다"), "{err}");
    git(m, &["checkout", "-q", "feature/timing"]);

    // (e) 빈 커밋 메시지 — 실패 + 한국어 안내. 단, stage_all 이 먼저 돌아
    //     파일이 스테이징된 채 남는다 (내용 손실은 아님 — FIXME(UX-9):
    //     실패한 커밋이 스테이징 부수효과를 남긴다, src/git/ops.rs:97-101.
    //     최소 수정: 메시지 공백 검증을 add -A 앞으로).
    write_file(m, "app.txt", "a-merged + 추가 작업\n");
    let c = checked(m, "빈 메시지 커밋", || commit(&t, "", true)).unwrap();
    assert!(!c.ok);
    assert_eq!(c.message, "커밋 메시지를 입력하세요.");
    let st = list_status(&t).unwrap();
    let f = st.files.iter().find(|f| f.path == "app.txt").unwrap();
    assert!(f.staged, "현재 동작: 실패한 커밋 뒤에도 스테이징은 남는다");

    // 제대로 커밋하고 push.
    let c = checked(m, "정상 커밋", || commit(&t, "추가 작업", true)).unwrap();
    assert!(c.ok);
    let p = checked(m, "push", || push(&t, None, None)).unwrap();
    assert!(p.ok);

    // (f) 푸시할 것 없음 — 오류가 아니라 "이미 최신" 성공.
    let p = checked(m, "push(할 것 없음)", || push(&t, None, None)).unwrap();
    assert!(p.ok, "빈 push 는 성공(최신) 취급: {}", p.message);

    // (g) 연속 동기화 멱등성 — 두 번째는 아무 커밋도 만들지 않는다.
    manager_commit_main(mgr.path(), "mgr_note.txt", "관리자 메모\n", "note");
    let sr = checked(m, "sync 1", || sync_to_base(&t, "main", "origin")).unwrap();
    assert!(!sr.conflicted, "{}", sr.message);
    let head1 = git(m, &["rev-parse", "HEAD"]);
    let sr = checked(m, "sync 2(연타)", || sync_to_base(&t, "main", "origin")).unwrap();
    assert!(!sr.conflicted);
    assert_eq!(git(m, &["rev-parse", "HEAD"]), head1, "연속 sync 는 멱등이어야 한다");
}

/// 시나리오 8 부록 — 커밋이 하나도 없는 저장소에서의 push.
/// git 의 "src refspec … does not match any" 를 사람 말로 바꾸는 코드가
/// 있지만(src/git/ops.rs:694-696) 실제로는 도달하지 못한다.
#[test]
fn s8b_push_on_unborn_repo_gives_misleading_detached_message() {
    let bare = TempDir::new().unwrap();
    git(bare.path(), &["init", "--bare", "-q", "-b", "main"]);
    let work = TempDir::new().unwrap();
    git(work.path(), &["init", "-q", "-b", "main"]);
    set_identity(work.path(), "팀원", "member@t.com");
    git(
        work.path(),
        &["remote", "add", "origin", &format!("file://{}", bare.path().display())],
    );
    write_file(work.path(), "draft.txt", "아직 커밋 전 초안\n");

    let t = target(work.path());
    // 회귀 방지(UX-6): unborn(커밋 없는) 저장소는 detached 로 오판하지
    // 않는다 — symbolic-ref 로 브랜치 이름을 얻어 push 를 시도하고, git 의
    // src-refspec 실패가 "푸시할 커밋이 없습니다" 한국어 안내로 온다.
    let out = checked(work.path(), "unborn push", || push(&t, None, None)).unwrap();
    assert!(!out.ok);
    assert!(
        out.message.contains("푸시할 커밋이 없습니다"),
        "올바른 처방(먼저 커밋)이어야 한다: {}",
        out.message
    );
    assert_eq!(read_file(work.path(), "draft.txt"), "아직 커밋 전 초안\n");
}

// ═════════════════════════════════════════════════════════════════════════
// 시나리오 9 — 적대적 내용: CRLF, 한글 경로, 충돌 마커를 닮은 본문.
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn s9_adversarial_content_survives_commit_sync_resolve() {
    let (rig, mgr) = new_rig(&[("readme.txt", "base\n")]);
    let mem = clone_member(&rig, "팀원", "member@t.com");
    let m = mem.path();
    let t = target(m);

    let crlf_body: &[u8] = b"windows line one\r\nwindows line two\r\n";
    let markers_body = "intro line\n<<<<<<< HEAD\nsample ours\n=======\nsample theirs\n>>>>>>> feature/sample\noutro line\n";
    let kfile = "한글 폴더/보고서 메모.txt";

    checked(m, "create_branch", || create_branch(&t, "feature/adversarial")).unwrap();
    write_bytes(m, "crlf.txt", crlf_body);
    write_file(m, kfile, "첫 줄 내용\n둘째 줄\n");
    write_file(m, "markers.md", markers_body);
    let c = checked(m, "적대적 내용 커밋", || commit(&t, "adversarial files", true)).unwrap();
    assert!(c.ok, "{}", c.message);
    // 커밋이 바이트를 바꾸지 않았는지 (CRLF 그대로, 마커 본문 그대로).
    assert_eq!(git_bytes(m, &["show", "HEAD:crlf.txt"]), crlf_body);
    assert_eq!(
        git_bytes(m, &["show", "HEAD:markers.md"]),
        markers_body.as_bytes()
    );
    let p = checked(m, "푸시", || push(&t, None, None)).unwrap();
    assert!(p.ok);

    // 관리자: 두 파일의 같은 줄을 다르게 고쳐 push → sync 에서 충돌 2건.
    manager_merge_and_push(mgr.path(), "feature/adversarial");
    set_line(mgr.path(), "markers.md", 0, "intro line (관리자)");
    set_line(mgr.path(), kfile, 0, "첫 줄 (관리자)");
    git(mgr.path(), &["add", "-A"]);
    git(mgr.path(), &["commit", "-q", "-m", "manager edits adversarial"]);
    git(mgr.path(), &["push", "-q", "origin", "main"]);

    set_line(m, "markers.md", 0, "intro line (팀원)");
    set_line(m, kfile, 0, "첫 줄 (팀원)");
    let c = checked(m, "팀원 충돌 편집 커밋", || commit(&t, "member edits", true)).unwrap();
    assert!(c.ok);

    let sr = checked(m, "sync(적대적 충돌)", || sync_to_base(&t, "main", "origin")).unwrap();
    assert!(sr.conflicted);
    assert!(
        sr.files.contains(&"markers.md".to_string())
            && sr.files.contains(&kfile.to_string()),
        "한글 경로가 이스케이프 없이 그대로 나와야 한다: {:?}",
        sr.files
    );

    // 마커 본문 파일의 3면 확인 — 내용으로서의 마커 줄이 살아 있다.
    let d = checked(m, "conflict_detail(markers.md)", || {
        conflict_detail(&t, "markers.md")
    })
    .unwrap();
    assert!(d.ours.contains("intro line (팀원)") && d.ours.contains("sample ours"));
    assert!(d.theirs.contains("intro line (관리자)"));

    // FIXME(BUG-3): 정당한 본문에 마커를 닮은 줄이 있으면 manual 해결이
    // 무조건 거부된다 — has_unresolved_markers(src/git/merge.rs)가
    // "내용으로서의 <<<<<<< " 와 "실제 미해결 마커"를 구분하지 못한다.
    // 회귀 방지(BUG-3): 원문(스테이지 :1/:2/:3)에 이미 있던 마커-닮은
    // 줄은 정당한 내용으로 허용된다 — 마커 예시가 담긴 문서도 manual 로
    // 병합할 수 있다. (원문에 없던 **새** 마커는 여전히 거부 — 아래 확인.)
    let novel = "intro line\n<<<<<<< NEW-NOVEL-MARKER\nx\n>>>>>>> other\n";
    let err = checked(m, "manual(새 마커) — 거부", || {
        resolve_conflict(
            &t,
            "markers.md",
            &Resolution::Manual { content: novel.to_string() },
        )
    })
    .unwrap_err();
    assert!(err.to_string().contains("충돌 표시"), "{err}");

    let remaining = checked(m, "manual(원문 마커 본문) — 허용", || {
        resolve_conflict(
            &t,
            "markers.md",
            &Resolution::Manual {
                content: markers_body.replace("intro line", "intro line (팀원)"),
            },
        )
    })
    .unwrap();
    assert_eq!(remaining, vec![kfile.to_string()]);

    // 한글 경로는 manual 해결 (마커 없는 본문은 정상 동작).
    let remaining = checked(m, "resolve manual(한글 경로)", || {
        resolve_conflict(
            &t,
            kfile,
            &Resolution::Manual { content: "첫 줄 (관리자+팀원)\n둘째 줄\n".into() },
        )
    })
    .unwrap();
    assert!(remaining.is_empty());
    let done = checked(m, "complete_merge", || complete_merge(&t, None)).unwrap();
    assert!(done.ok, "{}", done.message);

    // 병합 결과 검증: 마커-닮은 본문 줄이 전부 그대로 + ours 의 intro 줄.
    let merged = String::from_utf8(git_bytes(m, &["show", "HEAD:markers.md"])).unwrap();
    for line in ["<<<<<<< HEAD", "sample ours", "=======", "sample theirs", ">>>>>>> feature/sample", "outro line"] {
        assert!(merged.contains(line), "마커 본문 줄이 사라짐: {line}");
    }
    assert!(merged.contains("intro line (팀원)"));
    assert_eq!(
        String::from_utf8(git_bytes(m, &["show", &format!("HEAD:{kfile}")])).unwrap(),
        "첫 줄 (관리자+팀원)\n둘째 줄\n"
    );
    // CRLF 파일은 어느 단계에서도 변형되지 않았다.
    assert_eq!(git_bytes(m, &["show", "HEAD:crlf.txt"]), crlf_body);
    assert_eq!(fs::read(m.join("crlf.txt")).unwrap(), crlf_body);
}

// ═════════════════════════════════════════════════════════════════════════
// 발견 10 — [수정 완료·회귀 방지 BUG-7] fetch_origin 의 token 경로 제거.
// ═════════════════════════════════════════════════════════════════════════

/// 예전 `fetch_origin(repo, Some(token))` 은 origin URL 을
/// `https://oauth2:<token>@placeholder.invalid` 로 영구 교체(토큰 평문 유출 +
/// 이후 모든 원격 동작 파손)하고 실패를 삼켜 거짓 성공을 보고했다. token
/// 경로는 통째로 제거됐다 — run_pull_and_merge 는 URL 을 절대 건드리지 않고,
/// 실제 fetch 로 최신 origin/main 을 받아 병합한다.
#[test]
fn f10_run_pull_and_merge_never_touches_origin_url() {
    let (rig, mgr) = new_rig(&[("app.txt", "v1\n")]);
    let mem = clone_member(&rig, "팀원", "member@t.com");
    let m = mem.path();

    let url_before = git(m, &["remote", "get-url", "origin"]).trim().to_string();
    assert_eq!(url_before, rig.url);

    // 관리자가 main 을 전진시킨다 — 진짜 fetch 가 일어나면 이 커밋이 온다.
    write_file(mgr.path(), "app.txt", "v2 (관리자)\n");
    git(mgr.path(), &["add", "-A"]);
    git(mgr.path(), &["commit", "-q", "-m", "advance main"]);
    git(mgr.path(), &["push", "-q", "origin", "main"]);

    let sr = checked(m, "run_pull_and_merge", || {
        run_pull_and_merge(m, "origin", "main", None)
    })
    .expect("정상 동기화");
    assert!(!sr.conflicted);

    // URL 은 그대로, 토큰·placeholder 흔적 없음, 병합 결과는 최신이다.
    let url_after = git(m, &["remote", "get-url", "origin"]).trim().to_string();
    assert_eq!(url_after, url_before, "origin URL 은 절대 바뀌지 않는다");
    let cfg = std::fs::read_to_string(m.join(".git/config")).unwrap();
    assert!(!cfg.contains("placeholder.invalid") && !cfg.contains("oauth2:"));
    assert_eq!(
        std::fs::read_to_string(m.join("app.txt")).unwrap(),
        "v2 (관리자)\n",
        "낡은 트래킹 ref 가 아니라 실제 fetch 결과가 병합된다"
    );
}
